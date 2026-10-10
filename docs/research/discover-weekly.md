# Discover Weekly as a separate module: research and recommendation

Issue FLA-15. Research only: no code is changed. It answers the project's ask A10 in
`docs/architecture.md` ("per user, new songs that fit their taste, none already in any of their
playlists or likes"), which that document marks "partial".

Repo claims cite file and line on `origin/main` as of this branch. Lines drift; search for the
symbol if one is off. Facts about outside projects were read on 2026-10-10 from the URLs given.
Anything not confirmed is marked **unverified**.

## 1. The rule and why nothing in the repo meets it today

**Hard rule:** no song may appear that is already in any of that user's playlists or liked songs.

- Made for you does not try to meet it. Rediscover is built from songs the user already loved
  (`octo/Services/Library/ForYouLists.cs:85-86`, `:93-103`). Deep Cuts and Rediscover draw from the
  user's library walk only (`ForYouLists.cs:109-119`; `GeneratedPlaylistService.cs:352`). New
  Releases follows the user's top artists (`GeneratedPlaylistService.cs:384`).
- The only exclusion in that code is Deep Cuts leaving out what Rediscover chose
  (`GeneratedPlaylistService.cs:367-368`). Nothing reads the user's playlists or likes to exclude.
- Made for you lists are served by Octo and never written to Navidrome
  (`docs/architecture.md` section 3.4), so they cannot serve as the pattern for "appears in
  Navidrome".

The rule needs two sets built for each user, each week: their **playlists** and their **likes**.
How each can be read is the main design constraint (section 5.2).

## 2. Survey of open-source projects that could be integrated

Licence, last release and last push come from the GitHub API on 2026-10-10
(`https://api.github.com/repos/<owner>/<repo>`). "Runs as" says how it would sit beside the stack.

| Project | What it is | Licence | Maintenance | Runs as |
|---|---|---|---|---|
| [Troi](https://github.com/metabrainz/troi-recommendation-playground) (MetaBrainz) | The engine behind ListenBrainz "Weekly Jams" and "Weekly Exploration"; Python pipeline of elements, includes a local-collection resolver for Subsonic servers ([docs](https://troi.readthedocs.io/)) | GPL-2.0 (GitHub). | Release v2026.09.01.0 (2026-09-01); pushed 2026-09-28 | Python CLI/library. A new container, or run by hand. Needs the ListenBrainz API |
| [ListenBrainz server](https://github.com/metabrainz/listenbrainz-server) | The service itself; recommendations, similar recordings, "created for you" playlists | GPL-2.0 | Release 2026-10-08; pushed 2026-10-09 | Not self-hosted for this: Octo calls the public API (as `ListenBrainzRadioClient` already does) |
| [Explo](https://github.com/LumePart/Explo) (LumePart) | Self-hosted "Discover Weekly": pulls ListenBrainz Weekly Exploration, Weekly Jams and Daily Jams, requests the tracks from YouTube/Soulseek (or albums from Lidarr), tags them, creates playlists in Navidrome and others, on a schedule ([README](https://github.com/LumePart/Explo/blob/main/README.md)) | MIT | Release v1.2.0 (2026-09-09); pushed 2026-09-09 | Go container (Docker image and Helm chart in its repo), with its own web UI, its own yt-dlp and slskd client |
| [navidrome-listenbrainz-daily-playlist](https://github.com/kgarner7/navidrome-listenbrainz-daily-playlist) | Navidrome plugin that imports ListenBrainz daily/weekly playlists, matching by MusicBrainz id, per user; library songs only ([README](https://github.com/kgarner7/navidrome-listenbrainz-daily-playlist#readme)) | MIT | Pushed 2026-09-15 (release not checked) | WASM plugin inside Navidrome 0.63 or later; no new container |
| [AudioMuse-AI](https://github.com/NeptuneHub/AudioMuse-AI) | Audio analysis of the library, "rediscover forgotten songs", similarity playlists; Navidrome among several servers ([README](https://github.com/NeptuneHub/AudioMuse-AI#readme)) | AGPL-3.0 | Release v3.6.3 (2026-09-26); pushed 2026-09-27 | Docker Compose stack including PostgreSQL (its README); works on the existing library only, so it cannot propose songs the library lacks. Overlaps `octo-sonic` |
| [Gorse](https://github.com/gorse-io/gorse) | General recommender engine (collaborative filtering, item-based, popular/latest) with a REST API | Apache-2.0 | Release v0.5.11 (2026-07-14); pushed 2026-10-10 | Go service plus a database, its own container(s). Needs a feed of feedback from many users to learn; a household server has too few |
| [implicit](https://github.com/benfred/implicit) | Python library, collaborative filtering on implicit feedback | MIT | Release v0.7.3 (2026-05-08) | Library only. Same data problem as Gorse |
| [LensKit](https://github.com/lenskit/lkpy) | Python recommender research toolkit | MIT-style text in `LICENSE.md` (GitHub reports "other") | Release v2026.4.0 (2026-09-30) | Library only. Same data problem |
| [RecBole](https://github.com/RUCAIBox/RecBole) | Python recommender research framework | MIT | Last release v1.2.1 (2025-02-23); last push 2025-02-24, so it looks unmaintained | Library only. Same data problem |

Not used as candidates: [Essentia](https://github.com/MTG/essentia) (AGPL-3.0, audio analysis; the
repo already has `octo-sonic` for that) and [bliss-rs](https://github.com/Polochon-street/bliss-rs)
(GPL-3.0; `octo-sonic` already builds on it per the `Distance` comment in `SonicStore.cs`).

What the survey shows:

1. **Collaborative filtering libraries (Gorse, implicit, LensKit, RecBole) need many users' listening
   data.** Octo serves a handful of listeners. Running one would mean training on data the stack does
   not have. The big public dataset that does have it is ListenBrainz, which Octo can already reach.
2. **The only projects that produce "new songs for you" from a large dataset are ListenBrainz's own
   (Troi, run by MetaBrainz) and the tools that consume its output (Explo, the Navidrome plugin).**
   ListenBrainz exposes that output over its public API: per-user "created for" playlists, which are
   public and need no token (`GET /1/user/<name>/playlists/createdfor`,
   [docs](https://listenbrainz.readthedocs.io/en/latest/users/api/playlist.html)), and collaborative
   filtering recommendations (`GET /1/cf/recommendation/user/<name>/recording`,
   [docs](https://listenbrainz.readthedocs.io/en/latest/users/api/recommendation.html)). So Octo does
   not need to run Troi to use its results.
3. **Explo is the closest ready-made fit, but it duplicates what Octo does and does not meet the rule
   as stated.** It requests, downloads, tags and files songs itself; Octo already has Soulseek, YouTube
   and Lidarr acquisition with song-identity checks. Its README lists "smart deduplication" of
   downloads, but nothing says it checks a user's other playlists and likes (**unverified**; not found
   in the README). It is one more container holding its own downloader credentials, which `AGENTS.md`
   puts behind human approval, and whether it handles several Navidrome users is **unverified**.
4. **The Navidrome plugin is the lightest integration but cannot reach songs outside the library**, so
   it cannot make a list of new songs unless they are already there.

## 3. Building on what the repo already has, used read-only

"Read-only" means: the module calls these services and never changes them. Made for you's files stay
as they are.

| Existing piece | What it gives | Limits for Discover Weekly |
|---|---|---|
| `ListenBrainzRadioClient` (`octo/Services/ListenBrainz/ListenBrainzRadioClient.cs`) | `RecordingMbidAsync` (:34) turns an artist and title into a recording id; `SimilarRecordingsAsync` (:46) gives similar recordings, scores scaled 0-1; `LbRadioAsync` (:69) gives LB Radio for an artist or tag, with a token. Calls are limited to one a second (:22). | Seed-based ("like this song"), not "for this user". Has no call for a user's recommendations or created-for playlists; the module would add its own small client. The token is needed for LB Radio (:71) |
| `ListenBrainzSettings` (`octo/Models/Settings/ListenBrainzSettings.cs`) | Per-listener tokens (`UserTokens`, `TokenFor`) | Holds tokens, not ListenBrainz user names. A token's owner can be found with `GET /1/validate-token` ([docs](https://listenbrainz.readthedocs.io/en/latest/users/api/core.html)); that it returns the user name is **unverified**, check in FLA-23 |
| `LastFmService` (`octo/Services/LastFm/LastFmService.cs`) | `FindSimilarTracksAsync` (:78), `GetSimilarArtistTracksAsync` (:212), `GetArtistTopTracksAsync` (:415), `GetSimilarArtistsAsync` (:452), tag calls | Needs the server's API key. Only artist/track/tag methods; no per-user calls (no `user.*` method appears in `octo/Services/LastFm/`). Per-listener session keys exist for scrobbling (`LastFmSettings.UserSessions`, :38) |
| `octo-sonic` through `SonicStore` (`octo/Services/Sonic/SonicStore.cs`) | `Nearest` (:91) and `FitFactors` (:138): how close a song sounds to a seed | **Library songs only.** `SonicStore` holds features for songs read from `/music`; "a candidate without a sound is left out" (:132). A song that is not yet downloaded has no features, so sonic cannot propose it or rank it before it is fetched |
| `IRadioSource` set (`octo/Services/Radio/RadioTypes.cs:53`) | A common "songs like this seed" interface over Last.fm, ListenBrainz, YouTube Music and Sounds alike | Seed-based and shaped for a live radio queue (blend, spacing, bans). Reusing it would couple the module to radio behaviour. Calling the underlying clients directly avoids that |
| Acquisition (`TrackAcquisitionQueue`, `AcquisitionRequest` in `octo/Services/Common/`) | Turns an outside song into a library file: Soulseek, then YouTube, then Lidarr, in the configured order | Heavy, and the cost lands on disk and bandwidth, so it needs a per-user weekly cap |
| `SongIdentity.Same` (`octo/Services/Common/SongIdentity.cs:987`) | Compares two songs by title, artist, length and ISRC with a verdict and a confidence | Exactly what the exclusion rule needs; `ImportMatcher` already uses it for the same purpose (`ImportMatcher.cs:121`) |
| `NavidromePlaylistApi` (`octo/Services/Library/NavidromePlaylistApi.cs`) | As the admin: list all playlists (:35) with owner, read every track (:197), create a private playlist (:129), hand it to a user (:151), add (:111) and remove (:98) tracks | Admin identity, which `docs/security-requirements.md` R2.5 says not to use as a shortcut around ownership; see 5.4 |

**Conclusion of the comparison.** Everything needed to find and fetch candidates already exists in
the repo or is one public API call away. What does not exist is (a) a per-user taste input,
(b) the exclusion sets, (c) a weekly build, and (d) a Navidrome-owned playlist for outside songs.
Those four are the module.

## 4. Recommendation (one design)

**Build Discover Weekly as a new module in Octo that uses ListenBrainz and Last.fm as read-only
candidate sources, enforces the exclusion rule itself, fetches songs through Octo's existing
acquisition, and publishes one private Navidrome playlist per user. Do not adopt Explo, Troi or a
recommender library in v1.** No new container is needed, so nothing in this design needs the
human-approval step for a new service. Section 8 lists the optional additions that would.

### 4.1 Where candidate songs come from

In priority order, merged by a score of 0 to 1 per source:

1. **ListenBrainz "Weekly Exploration" for that user** (public created-for playlist, no token).
   It is ListenBrainz's own answer to this exact feature, built from the user's listens. Needs the
   user's ListenBrainz name (section 5.1).
2. **ListenBrainz collaborative-filtering recommendations for that user** (same name required).
3. **Seeded similarity.** For about ten seeds drawn fresh each week from what the user loved recently
   (hearted, rated 4 or more, or played often; the thresholds Made for you uses are `LovedPlays` 3
   and `LovedRating` 4 in `ForYouLists.cs:34-35`; the module keeps its own copies rather than reading
   those private fields): `SimilarRecordingsAsync` from ListenBrainz, and `FindSimilarTracksAsync` /
   `GetSimilarArtistTracksAsync` from Last.fm when a key exists. This also works for users with no
   ListenBrainz account.
4. **Library songs this user has never touched** (other people's music on a shared server). These
   need no download. They are eligible only if they pass the same exclusion check. Sonic `FitFactors`
   may re-rank this group (the only group it can see).

"New songs" means: not in the user's playlists, not liked, and not shown to them in an earlier
Discover Weekly (a per-user ledger of past weeks, section 5.3). It also drops songs the user has
played at least once, because a played song is not a discovery. That second filter is a choice, not
part of the hard rule; the playlist/likes filter is the hard rule.

Why not `octo-sonic` as a candidate source: it can only see downloaded songs (above). It stays a
re-ranker for group 4 and, later, a post-download check.

### 4.2 How the exclusion rule is enforced

The rule is a gate, not a preference. Three layers, each independent, so a bug in one does not
publish a violation:

1. **Build the exclusion set for the user** immediately before selecting:
   - **Playlists:** every playlist the user owns or can see, every track, from
     `NavidromePlaylistApi.ListPlaylistsAsync` and `ReadAllTracksAsync`. The result also carries
     Octo's own playlists for the user (action playlists, Review notices, imports kept as playlists):
     all count. Only the user's current Discover Weekly playlist is left out of the set, because the
     module is about to replace its contents; its songs go to the ledger instead.
   - **Likes:** `rest/getStarred2` as the user (songs, and the songs of starred albums).
   - Each entry keeps artist, title, length, any ISRC and MusicBrainz recording id, and the Navidrome
     id.
2. **Select by identity, not id.** A candidate is dropped if any entry matches by MusicBrainz
   recording id, by ISRC, or by `SongIdentity.Same(...)` with default (strict) options. Outside songs
   have no Navidrome id, so id comparison alone would let a liked song back in under another
   spelling or version. A near match (`SameSongDifferentVersion`) also drops it: a user who liked
   the studio version does not want the live one as a "discovery". This is the same call
   `ImportMatcher.cs:121` uses.
3. **Re-check at the moment of writing.** Fetching takes minutes to hours; users keep liking songs.
   Right before every add to the playlist the module re-reads both sets, by the downloaded file's
   Navidrome id and again by identity, and skips the song if it matches. A weekly sweep also removes
   from the live playlist any track the user has since liked or placed in another playlist (it
   removes it from Discover Weekly only, never from their other lists).

**Fail closed.** If either set cannot be read in full (Navidrome error, partial page, expired
credential), the module publishes nothing for that user that run and tries again later. An unread
set is never treated as an empty one (compare `ReadAllTracksAsync`, which returns null on failure
for this reason: `NavidromePlaylistApi.cs:195-196`).

**Tests** (all in `octo.Tests/DiscoverWeekly/`, none needing a server; Navidrome and the catalogs
are stubbed as the existing tests do):

- Table test: a song in a playlist, a liked song, a liked album's song, the same song with a
  different spelling, remaster, "explicit" tag, or ISRC only: none can be selected.
- Property test: for random pools and random exclusion sets, no selected song matches any exclusion
  entry (the invariant, not examples).
- A live version and a remix of a liked song are excluded (version verdict).
- Exclusion set unreadable (null, error, truncated page): no playlist is written, and the previous
  week's playlist is left as it was.
- A song liked between selection and write is not added (layer 3).
- Playlist-owned-by-another-user songs are not in this user's set, and this user's set is not used
  for another (R2.7 in `docs/security-requirements.md`).
- A final end-to-end check over the written playlist: read it back from the stubbed Navidrome and
  assert the invariant again.

### 4.3 Weekly schedule

- A week is an ISO week in UTC. A hosted worker sweeps hourly (as `NoticePlaylistWorker` loops,
  `NoticePlaylistWorker.cs:123-139`) and, for each opted-in user whose last build is not this week's
  and who is due, runs a build. The list for a week lands after Monday 00:00 UTC; the first build may
  start any time that week.
- Fetching is spread out. The module hands songs to acquisition at no more than a set number per
  hour and per user (the same idea as `TrickleQueue` for imports, `octo/Services/Imports/TrickleQueue.cs`).
  Songs appear in the playlist as they land (Navidrome has to scan them first; the Notice worker
  already resolves a downloaded file to a Navidrome id by path with
  `NavidromeSongPathResolver.FindIdByPathAsync`, `NoticePlaylistWorker.cs:172`, so the module
  reuses that call).
- Default target 30 songs per user per week, at most 20 to fetch from outside, configurable. If fewer
  than a minimum (default 10) land by Sunday, the playlist keeps what it has; it is never padded
  with songs that fail the rule.
- A new week replaces the old contents in place. The old songs go to the ledger; they stay in the
  library (the module never deletes files; see 7).
- All of the module's work stops with the switch off, including the worker's sweeps.

### 4.4 How the list appears in Navidrome per user

One **private playlist named "Discover Weekly", owned by the user**, in Navidrome itself. It is
therefore visible in every Subsonic client and in Navidrome's own UI, through the proxy without any
change to `SubSonicController` (unlike Made for you, which Octo injects into responses). Creation
copies `LibraryActionPlaylistProvisioner` (`octo/Services/Library/LibraryActionPlaylistProvisioner.cs`
header and its `rest/createPlaylist` call): created with the user's own credentials, so it is theirs
and private, never public. Outside songs can be in a Navidrome playlist only once they are library
files, which is why fetching comes first and the playlist fills as songs land.

Cover and description: a description line "New this week, nothing you already have", set on creation.
A generated cover is out of scope for the research; the user can set one in a client.

## 5. Design details that decide the cost

### 5.1 Per-user taste input without a database

Octo has no user database (`docs/architecture.md` section 4.1). Taste comes from:

- the user's **Navidrome view as that user** (stars, ratings, play counts, last played), read with
  their credentials as Made for you does (`GeneratedPlaylistService.WalkLibraryAsync`, :442);
- the user's **ListenBrainz account**, when they have one: the existing per-listener token
  (`ListenBrainzSettings.UserTokens`) identifies it; if `validate-token` does not return the name,
  the module has its own `DiscoverWeekly:ListenBrainzUsers` map from Navidrome name to ListenBrainz
  name, set in the dashboard.

A user with neither still gets seeded-similarity candidates (4.1 item 3).

### 5.2 The credential problem

Likes of another user cannot be read with the admin identity: Navidrome keeps stars per user and
Octo's admin token (`NavidromePlaylistApi.SendAsync`, :228) acts as the admin. (That the native
API has no way to read another user's stars is **unverified**; no call in the repo does it, and
Octo's own code reads stars with each user's sign-in.) Octo only sees a user's credentials during
that user's requests.

Proposal, in the pattern the repo already has: hold the user's sign-in **in memory only**, as a
token never a password, and drop it after a day, exactly like `StarOnArrival`
(`octo/Services/Common/StarOnArrival.cs:23, 56, class doc`). A build for a user starts when a held
credential exists and their week is due. A user who does not open a client in a week gets the list
the next time they do. It is not guaranteed to be ready on Monday morning; that trade-off is
inherent to not storing passwords, and the alternative (storing a credential) conflicts with the
secrets requirements in `docs/security-requirements.md` section 1.

How the module gets the credential without editing the Subsonic controller is an open design point
for FLA-23 (**unverified that it works**): register an ASP.NET `IStartupFilter` or a result filter
from the module's own extension method. If that proves unworkable, the fallback is one call beside
the existing `Popular()` and `_generatedPlaylists.ListAsync(...)` calls
(`SubSonicController.cs:389-391`), which is a hotspot edit needing a claim first (`AGENTS.md`).

### 5.3 State

Two files in `/app/config`, both owned by the module and keyed by user (R2.2): `discover-weekly.json`
(per user: last build week, current playlist id, the week's picks and state) and a **ledger** of every
song ever shown to that user, by identity, so a song never repeats. Bounded (for example the last
2,000 entries per user).

### 5.4 Ownership and who writes the playlist

R2.5 says playlist changes use the caller's own credentials, not an admin shortcut. The playlist
creation and the first fill happen inside the build, with the held user credential. Songs that land
hours later may arrive when that credential is gone; then the module follows what
`NoticePlaylistWorker` already does (it fills user-owned private playlists as the admin, so the
playlist stays the user's: `NavidromePlaylistApi.cs:10-16`, `NoticePlaylistWorker.cs:183-185`), and
the code states why in a comment as R2.4 asks. This is an existing exception, not a new one, and
Security should confirm it for this module in review.

## 6. Module boundary

Per `docs/modules.md` (on the FLA-8 branch; the file is not yet on `main`):

| Part | Value |
|---|---|
| Folder | `octo/Services/DiscoverWeekly/`; namespace `Octo.Services.DiscoverWeekly`. Holds the settings class, the ListenBrainz recommendations client, the selector (pure, with the exclusion gate), the store and ledger, the worker, and `DiscoverWeeklyModule.cs` |
| Switch | `DISCOVER_WEEKLY_ENABLED`, default **off** for the first release (it downloads files). Four places: `.env.example`, `docker-compose.yml` (`DiscoverWeekly__Enabled`), `octo/appsettings.json`, `DiscoverWeeklySettings.Enabled`. Read at run time through `IOptionsMonitor`, not at registration |
| Registration | One line in `octo/Program.cs`: `builder.Services.AddDiscoverWeeklyModule(builder.Configuration);` |
| Tests | `octo.Tests/DiscoverWeekly/` (section 4.2) |
| Optional | Its own `octo/Controllers/DiscoverWeeklyController.cs` (admin and per-user "off"/status endpoints) and dashboard files under `octo/wwwroot` |
| Off means off | No outgoing calls, no work, no files written; endpoints say "off", not 500 |
| Made for you | **No edits.** The module reads nothing from `ForYouLists`, `GeneratedPlaylistService` or `NewReleasesBuilder`, and does not change `GeneratedPlaylistSettings`. It copies the "loved" thresholds rather than sharing them. Both can be on at once |
| Shared code | Used through public services only: `NavidromePlaylistApi`, `ListenBrainzRadioClient`, `LastFmService`, `SongIdentity`, `NavidromeSongPathResolver`, `TrackAcquisitionQueue`. If one lacks a call the module needs, the module adds its own, or a separate issue proposes the change |
| Hotspots to claim (expected) | `octo/Program.cs` (one line), `.env.example`, `docker-compose.yml`, `octo/appsettings.json`, `README.md` (one paragraph). `SubSonicController.cs` only if 5.2's fallback is needed |

The module is removable by the five steps in `docs/modules.md` "Removing a module".

## 7. Security and cost notes

- **Per-user isolation (R2.1-R2.8):** the user is taken from the verified sign-in; the store filters
  by user on every method; the isolation test in 4.2 covers A-reads-B. One user's picks are never
  placed in something another user can read.
- **External input:** candidate artist and title strings from ListenBrainz and Last.fm are untrusted;
  they go to the search and acquisition paths that already validate them (the shim and Soulseek
  rules in section 4 of `docs/security-requirements.md`). Never build a path or command from them.
- **No file deletion in v1.** Songs are downloaded into the library; ones the user does not like
  stay there, as hearted-then-unhearted songs do today. A clean-up of unwanted discoveries would be
  a deletion feature (R5.1-R5.8 apply: quarantine, dry-run first, hard cap, journal) and needs human
  approval before it runs on real data. It is a separate issue, not part of this module.
- **Cost:** with a cap of 20 outside songs per user a week, a three-person server takes about 60
  songs a week. The cap and the per-hour rate are settings.
- **Legal and terms:** automatic download of recommended songs has the same footing as the existing
  heart-to-fetch path, but at a volume nobody chose song by song. The human should decide whether
  that is acceptable before the switch defaults on. Default off, and the dashboard says what it does.
- **Rate limits:** ListenBrainz allows about one call a second (`ListenBrainzRadioClient.cs:22`);
  a ten-seed build is a few dozen calls. Fine.
- **Deployment:** GitHub Actions is disabled on this fork, so no CI result backs this document.

## 8. What needs human approval or a decision

1. **Nothing in the recommendation is a new service or container.** If a later phase wants Explo or
   Troi in the stack, that is a new container and needs written approval (`AGENTS.md` "Human
   approval"). Troi (GPL-2.0) could also be reviewed for licence fit before any linking.
2. **Automatic downloading at a weekly volume** (4.3, 7): confirm the default caps and that the
   feature ships off by default.
3. **Credential holding** (5.2): confirm in-memory, one-day holding of a user token, as `StarOnArrival`
   does, is acceptable; and whether the SubSonicController hotspot edit may be used if the
   no-edit route fails.
4. **ListenBrainz user names** (5.1): confirm a dashboard map is acceptable if `validate-token`
   does not return one.
5. **Admin fill of user-owned playlists** (5.4): Security to confirm it matches R2.4/R2.5 for this
   module.

## 9. Open items for the build issue (FLA-23)

- Confirm what `GET /1/validate-token` returns, and that created-for playlists can be listed for a
  user by name.
- Spike the `IStartupFilter` route for capturing the credential without touching the controller.
- Confirm Navidrome's playlist APIs behave as the existing workers assume when a track list is
  long (the helpers page at 500, `NavidromePlaylistApi.cs:200`).
- Decide the playlist cover and description wording.
