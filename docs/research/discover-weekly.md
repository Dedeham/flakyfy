# Discover Weekly as a separate module: research and recommendation

Issue FLA-15. Research only: no code is changed. It answers the project's ask A10 in
`docs/architecture.md` ("per user, new songs that fit their taste, none already in any of their
playlists or likes"), which that document marks "partial".

Repo claims cite file and line on `origin/main` as of this branch. Lines drift; search for the
symbol if one is off. Facts about outside projects were read on 2026-10-10 from the URLs given.
Anything not confirmed is marked **unverified**.

Two human decisions of 2026-10-10 shape this version (quoted on FLA-15): **D7**, no credential is held
for any length of time; the list is built in the user's first live request of the week (4.3, 5.2, 5.4).
**D8**, auto-downloads for each user, capped at 10 songs per user per week, the module switch off by
default, and a starred artist does not exclude all of its songs (4.2, 4.3). Section 10 re-checks
Security's 12 conditions against this design.

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
| [navidrome-listenbrainz-daily-playlist](https://github.com/kgarner7/navidrome-listenbrainz-daily-playlist) | Navidrome plugin that imports ListenBrainz daily/weekly playlists, matching by MusicBrainz id, per user; library songs only ([README](https://github.com/kgarner7/navidrome-listenbrainz-daily-playlist#readme)) | MIT | Release v6.0.0 (2026-07-08); pushed 2026-09-15 | WASM plugin inside Navidrome 0.63 or later; no new container |
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
   - **Playlists:** the playlists the user can see, which are the ones they own plus other
     people's public ones. Read as the user: `rest/getPlaylists` returns exactly that set (Subsonic
     API: own and public playlists), then `rest/getPlaylist` per playlist for its songs, paged to
     the end. This is chosen because the existing admin helper cannot be used as written:
     `NavidromePlaylistApi.ListPlaylistsAsync` (`NavidromePlaylistApi.cs:35-49`) returns an empty
     list on any error, asks for one page of at most 1000 (`_end=1000`) with no paging, and
     `PlaylistRow` has only `Id`, `Name` and `Owner` (`LibraryActionPlaylistWorker.cs:189`,
     `ParsePlaylists` :192-204), with no public flag. A failed or truncated listing from it would
     look like "no playlists". The module therefore has its own reader that pages until a short page,
     validates the shape of each page (a non-array answer is a failure, unlike `ParsePlaylists`
     :194 and `ParseTracks`, which turn it into an empty list), and returns **null (unread)** on any
     error, non-success status, odd shape, or a page count it cannot confirm. The result includes
     Octo's own playlists for the user (action playlists, Review notices, imports kept as
     playlists): all count. Only the user's current Discover Weekly playlist is left out, because
     the module is about to replace its contents; its songs go to the ledger first (4.3).
   - **Likes:** `rest/getStarred2` as the user returns starred songs, starred albums and starred
     artists. Songs are likes. A starred album's songs are read with one `rest/getAlbum` per album,
     and all of them are likes. A starred artist is **not** treated as every song by that artist: the
     star means "I like this artist", not "I have heard all of it", and excluding the whole catalogue
     of the artists a user loves would remove the best discoveries (new songs by artists they like).
     Their individual songs are excluded only if starred, in an album they starred, or in a
     playlist. This is a decision for the human to confirm (section 8). Any failed `getAlbum`, or a
     `getStarred2` that fails or looks truncated, makes the whole likes set unread (null).
   - Each entry keeps artist, title, length, any ISRC and MusicBrainz recording id, and the Navidrome
     id.
2. **Select by identity, not id.** A candidate is dropped if any entry matches by MusicBrainz
   recording id, by ISRC, or by `SongIdentity.Same(...)` with default (strict) options. Outside songs
   have no Navidrome id, so id comparison alone would let a liked song back in under another
   spelling or version. A near match (`SameSongDifferentVersion`) also drops it: a user who liked
   the studio version does not want the live one as a "discovery". This is the same call
   `ImportMatcher.cs:121` uses.
3. **Re-check at the moment of writing.** Fetching takes minutes to hours; users keep liking songs.
   Every write is made inside a run that has a fresh credential (5.2, 5.4). A run reads both sets
   first, and a top-up run for songs that landed later reads them **again** before it adds them,
   checking the downloaded file's Navidrome id and again by identity, and skips a song that matches.
   A run also sweeps the live playlist and removes any track the user has since liked or placed in
   another playlist (from Discover Weekly only, never from their other lists). Because no credential is
   kept, a song that lands when the user is not active simply waits; it is never added without a
   check, and the admin identity is not used to add (5.4).

**Fail closed.** Unread means publish nothing. If either set is null (Navidrome error, partial
page, a failed per-playlist or per-album call, a refused credential), the module publishes nothing for that user that run and tries again at their next live request. An unread
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
- Isolation (R2.7 in `docs/security-requirements.md`): user A builds data; user B cannot read,
  change or delete A's playlist id, ledger, picks or credential by id; the store holds no credential and has no
  cross-user method reachable from a controller; and B's exclusion set never includes A's private
  playlists.
- A song that lands when the user is not active: not added until a live request, then re-checked
  against both sets with that request's credential. The admin identity never writes to the playlist.
- No credential is kept: after a run, no module type holds a `SubsonicCredential` (checked by
  reflection over the module's types), and a request whose sign-in `CredentialCheck` refuses starts
  no run.
- Trigger: a user already built this week, an opted-out user, a name not allowed, an API-key sign-in
  and a request with no `u` start nothing; a user not active that week gets no run.
- A public Discover Weekly playlist after creation is refused and reported.
- A final end-to-end check over the written playlist: read it back from the stubbed Navidrome and
  assert the invariant again.

### 4.3 Weekly schedule

- A week is an ISO week in UTC. **There is no timer that builds lists.** A user's weekly run starts in
  their first live request of that week (5.2), so the list lands whenever they first open a client
  after Monday 00:00 UTC. A user who is not active that week gets no new list, and last week's list
  stays as it was.
- Fetching is spread out. The run hands the cleared outside songs to acquisition at no more than a set
  number per hour and per user (the same idea as `TrickleQueue` for imports,
  `octo/Services/Imports/TrickleQueue.cs`). Songs appear in the playlist at the user's next live
  request after they land (5.4); a credential-free worker resolves a downloaded file to a Navidrome
  id by path with `NavidromeSongPathResolver.FindIdByPathAsync` (as `NoticePlaylistWorker.cs:172`
  does) so a top-up only has to read the stored ids.
- **Targets (D8).** Up to 30 songs per user per week, of which **at most 10 are fetched from outside
  per user per week**; the rest come from library songs the user has never touched (4.1 item 4). The
  10 is a setting. On top of it there is a **server-wide weekly cap**, a per-hour rate, a
  free-disk-space floor that stops fetching when it is crossed, and a maximum file size and duration
  through the existing acquisition checks (`docs/security-requirements.md` R4.8). Nothing is fetched
  while the exclusion set is unread. If fewer than a minimum (default 10) are ready by Sunday, the
  playlist keeps what it has; it is never padded with songs that fail the rule.
- Auto-downloads apply to each allowed user when the module is on (D8). The module switch is **off by
  default**. A per-user opt-out and the allow-list pattern of `LibraryActionPlaylistProvisioner`
  (`settings.IsAllowed(username)`, `LibraryActionPlaylistProvisioner.cs:48`) decide who is covered.
- A new week replaces the old contents in place. **Order:** first the old playlist's songs and the
  new picks are written to the ledger and saved, then the old playlist is replaced. A crash between
  the two steps leaves the old list showing and a ledger that already counts it, never a replaced
  playlist whose songs are missing from the ledger. The old songs stay recorded; they stay in the
  library (the module never deletes files; see 7).
- A failed run (an exclusion set unread, Navidrome unreachable) changes nothing, is recorded as a
  count, and is retried at the user's next live request, no sooner than a back-off, never by a timer.
- All of the module's work stops with the switch off, including the credential-free worker and any run
  in flight.

### 4.4 How the list appears in Navidrome per user

One **private playlist named "Discover Weekly", owned by the user**, in Navidrome itself. It is
therefore visible in every Subsonic client and in Navidrome's own UI, through the proxy without any
change to `SubSonicController` (unlike Made for you, which Octo injects into responses). Creation
copies `LibraryActionPlaylistProvisioner` (`octo/Services/Library/LibraryActionPlaylistProvisioner.cs`
header and its `rest/createPlaylist` call): created with the user's own credentials, so it is theirs
and private, never public. The module then reads the playlist back and verifies the owner and that it is not
public; if it is public, it fails closed and reports. It never writes another user's picks into it.
Outside songs can be in a Navidrome playlist only once they are library
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
  name. Ownership proved by `validate-token` for the user's own stored token is preferred. The map is
  an admin-only setting under `/api/admin`, shows which name is mapped to whom, and validates length
  and allowed characters (the name is URL-encoded in paths). A user cannot type their own
  ListenBrainz name unproved, because a wrong entry would fill one person's list from another's
  listening history. A mapping is used only for public ListenBrainz data.

A user with neither still gets seeded-similarity candidates (4.1 item 3).

### 5.2 The credential problem and the trigger

Likes of another user cannot be read with the admin identity: Navidrome keeps stars per user and
Octo's admin token (`NavidromePlaylistApi.SendAsync`, :228) acts as the admin. (That the native
API has no way to read another user's stars is **unverified**; no call in the repo does it, and
Octo's own code reads stars with each user's sign-in.) Octo only sees a user's credentials during
that user's requests.

**Decision D7 (2026-10-10): no credential is held, not even for a day.** The list is built during the
user's **first live request of the week**, with the credential that request carried and Navidrome has
just accepted. The run uses it to read the user's likes and playlists as that user, then to create or
update the private playlist as that user. When the run ends the credential is gone. A user who does
not open a client in a given week gets no new list that week; the next time they do, the run happens.
This removes the memory-resident token an earlier version of this design held: there is nothing for a memory read
to find that a request in flight does not already hold, and `StarOnArrival`'s day-long holding is not
copied.

**How a run starts, without a global `IStartupFilter` doing the work.** The module registers one small
middleware from its own extension method (`AddDiscoverWeeklyModule`), restricted to `/rest/` paths
with `UseWhen`. It does no work on the request path beyond one in-memory lookup:

1. Read `u` and the sign-in parameters from the query string (`SubsonicCredential.From`,
   `octo/Services/Subsonic/SubsonicCredential.cs`). No `u` (an API-key sign-in) means stop.
2. Ask the module's store, in memory, whether this name is due: module on, name allowed and not
   opted out, no run in flight for it, last build week is not the current ISO week (or there is
   landed pending work for a top-up), and the last failed attempt was not within the back-off. This
   uses the unverified `u` only to decide whether to bother; it acts on nothing.
3. If due, hand a copy of the credential to a background task (through `WithoutPassword()`, so a
   password is not kept) and let the request carry on. The request is never delayed.
4. The task first asks `CredentialCheck.CheckAsync` (`octo/Services/Subsonic/CredentialCheck.cs`),
   which pings Navidrome with that sign-in and answers Accepted, Refused or Unreachable. **Only
   Accepted starts a run.** The user name is the `u` of that same accepted credential; it is never
   taken from a header, a body field or another request (R2.1). A refused or unreachable answer
   starts nothing and is not retried by the module (the next request tries again, and the existing
   `SignInThrottle` and the 30-second refusal memory in `CredentialCheck` limit the cost).
5. The run reads the sets, selects, creates or updates the playlist and queues downloads, all as that
   user, then ends and drops the credential. It is cancelled at once if the switch goes off or the
   user opts out, and has a time limit.

What the credential may do: it is used for that user only and only inside the run; it is never put in
a field of a store, worker, job record, log line, exception message, notification or dashboard
answer, and is never serialized. A test (4.2) asserts that no module type keeps a `SubsonicCredential`
after a run.

What the first request must be: "first activity of the week" means the first request that passes the
steps above, of any kind (a ping, a getPlaylists, a stream). It is not limited to one endpoint.

**Why a module filter is expected to work without a hotspot edit (unverified).** The existing
precedent for "act on the user's live, accepted request" is in the controller:
`GetPlaylists` relays to Navidrome, checks the answer was successful, and then calls
`_actionPlaylists.EnsureAsync(username, PlaylistNames(...), parameters)` without waiting
(`SubSonicController.cs:361-381`); `LibraryActionPlaylistProvisioner.EnsureAsync` then creates the
user's playlists with the user's own credentials. A module filter that does the same without touching
that file is the preferred route (Security condition 3). Points the FLA-23 spike must settle:
(a) that an `IStartupFilter` added to the service collection by the module extension runs under the
`WebApplication` that `Program.cs` builds; (b) that reading the query string there does not disturb the
body buffering middleware at `Program.cs:725`, so a POST'd form sign-in is simply not seen by the
filter (such a client triggers a run on its next query-string request, or not at all, which the spike
reports); (c) that the work is started in the background.

**Fallback, a hotspot edit.** If (a) to (c) cannot be met, the fallback is one line beside the existing
`EnsureAsync` call in `GetPlaylists` (`SubSonicController.cs:379-381`): it already has the parsed
parameters (query and form) and a request Navidrome has just accepted, so no `CredentialCheck` ping
is needed. It changes only that call and needs a claim on `SubSonicController*.cs` first (`AGENTS.md`
hotspots). The earlier candidate, the lines beside `Popular()` and `_generatedPlaylists.ListAsync(...)`
(`SubSonicController.cs:389-391`), is no longer the one to use: it belongs to the Made for you
injection and the module should not sit in it.

**API keys.** An OpenSubsonic API-key sign-in carries no `u` (`SubsonicCredential.User` is null), so
the module cannot name the user without a further `tokenInfo` call (`RequestIdentity`). In v1 such a
request starts nothing, and the module's status shows "your client signs in with an API key". This is
the same outcome Security's condition 2 asked for, reached without a hold (section 8).

**What this costs the user.** The list is not ready on Monday morning, and not at all in a week the
user never opens a client. That is inherent to not storing a credential
(`docs/security-requirements.md` section 1) and is accepted by the human (D7).


### 5.3 State

Two files in `/app/config`, both owned by the module and keyed by user (R2.2): `discover-weekly.json`
(per user: last build week, current playlist id, the week's picks and state) and a **ledger** of every
song ever shown to that user, by identity, so a song never repeats. Both are created with mode 0600
from the start and written to a temporary file that is then moved over the old one, so a crash
never leaves a half-written file or a window where it is readable by others. Each is bounded at 2,000
entries per user. A user's rows are removed when they opt out, and all of them when the module is
turned off and the admin chooses to clear. Neither holds tokens or credentials. Logs and the
dashboard status carry counts, not song lists tied to a user name, unless shown only to that user.

### 5.4 Ownership and who writes the playlist

R2.5 says playlist changes use the caller's own credentials, not an admin shortcut. With the
live-request design that holds with no exception: **every write to a user's Discover Weekly playlist
is made as that user, inside a run that has just read both exclusion sets.** There is no admin fill
and no gap between clearing a song and adding it.

- A run creates or updates the playlist and adds the songs that are already in the library and passed
  the check. For songs the module started to fetch, it stores only the candidate's identity (no
  credential) in the user's pending list.
- A small credential-free worker resolves landed files to Navidrome ids by path
  (`NavidromeSongPathResolver.FindIdByPathAsync`, as `NoticePlaylistWorker.cs:172` does) and marks
  them landed in the store. It never writes to the playlist.
- A landed song is added at the user's **next live request**: a top-up run reads both sets again
  with that request's fresh credential, drops any landed song that now matches, and adds the rest.
  This is the "late-landing songs wait for the user's next live request" rule. A user who does not
  open a client sees them the next time they do.
- The admin identity (`NavidromePlaylistApi`) is used for nothing here beyond the credential-free
  path lookup. The `NoticePlaylistWorker` exception (it fills user-owned playlists as the admin,
  `NavidromePlaylistApi.cs:10-16`, `NoticePlaylistWorker.cs:183-185`) is not extended to this module.


## 6. Module boundary

Per `docs/modules.md` (on the FLA-8 branch; the file is not yet on `main`):

| Part | Value |
|---|---|
| Folder | `octo/Services/DiscoverWeekly/`; namespace `Octo.Services.DiscoverWeekly`. Holds the settings class, the ListenBrainz recommendations client, the selector (pure, with the exclusion gate), the store and ledger, the worker, and `DiscoverWeeklyModule.cs` |
| Switch | `DISCOVER_WEEKLY_ENABLED`, default **off** for the first release (it downloads files). Four places: `.env.example`, `docker-compose.yml` (`DiscoverWeekly__Enabled`), `octo/appsettings.json`, `DiscoverWeeklySettings.Enabled`. Read at run time through `IOptionsMonitor`, not at registration |
| Registration | One line in `octo/Program.cs`: `builder.Services.AddDiscoverWeeklyModule(builder.Configuration);` |
| Tests | `octo.Tests/DiscoverWeekly/` (section 4.2) |
| Optional | Its own `octo/Controllers/DiscoverWeeklyController.cs` (admin and per-user "off"/status endpoints) and dashboard files under `octo/wwwroot` |
| Off means off | No outgoing calls, no work, no runs, no files written; endpoints say "off", not 500 |
| Made for you | **No edits.** The module reads nothing from `ForYouLists`, `GeneratedPlaylistService` or `NewReleasesBuilder`, and does not change `GeneratedPlaylistSettings`. It copies the "loved" thresholds rather than sharing them. Both can be on at once |
| Shared code | Used through public services only: `NavidromePlaylistApi`, `ListenBrainzRadioClient`, `LastFmService`, `SongIdentity`, `NavidromeSongPathResolver`, `TrackAcquisitionQueue`. If one lacks a call the module needs, the module adds its own, or a separate issue proposes the change |
| Hotspots to claim (expected) | `octo/Program.cs` (one line), `.env.example`, `docker-compose.yml`, `octo/appsettings.json`, `README.md` (one paragraph). **`SubSonicController.cs` is not expected to need an edit**: the trigger is a filter the module registers itself (5.2). Only if the FLA-23 spike shows that fails, one line beside `EnsureAsync` in `GetPlaylists`, after a claim |

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
- **Cost:** with a cap of 10 outside songs per user a week, a three-person server takes about 30
  songs a week, and only for users who open a client that week. The per-user cap, the server-wide
  weekly cap, the per-hour rate and the disk floor are settings.
- **Legal and terms:** automatic download of recommended songs has the same footing as the existing
  heart-to-fetch path, but at a volume nobody chose song by song. The human decided that it is on for
  each user with a cap of 10 a week and the module switch off by default (D8). Default off, and the
  dashboard says what it does.
- **Rate limits:** ListenBrainz allows about one call a second (`ListenBrainzRadioClient.cs:22`);
  a ten-seed build is a few dozen calls. Fine.
- **Deployment:** GitHub Actions is disabled on this fork, so no CI result backs this document.

## 8. What needs human approval or a decision

Decided on 2026-10-10 (quoted on FLA-15) and no longer open: D7, no credential is held, the list is
built in the user's first live request of the week (5.2); D8, auto-downloads for each user, capped at
10 songs per user per week, switch off by default, and a starred artist does not exclude all of its
songs (4.2, 4.3). Still open:

1. **Nothing in the recommendation is a new service or container.** If a later phase wants Explo or
   Troi in the stack, that is a new container and needs written approval (`AGENTS.md` "Human
   approval"). Troi (GPL-2.0) could also be reviewed for licence fit before any linking.
2. **Server-wide caps** (4.3): confirm the defaults for the server-wide weekly cap, the per-hour rate
   and the free-disk floor. The per-user cap of 10 is decided.
3. **The SubSonicController hotspot** (5.2): may the one-line fallback beside `EnsureAsync` in
   `GetPlaylists` be used if the no-edit filter route fails the FLA-23 spike?
4. **ListenBrainz user names** (5.1): confirm an admin-only map is acceptable if `validate-token`
   does not return a name.
5. **Users whose client sends an API key** (5.2): they get no Discover Weekly in v1. Confirm that is
   acceptable, or ask for a later issue to support them.
6. **Opt-out** (D8 says "each user"): confirm that every allowed user is covered by default once the
   module is on, with a per-user opt-out, rather than opt-in.

## 9. Open items for the build issue (FLA-23)

- Confirm what `GET /1/validate-token` returns, and that created-for playlists can be listed for a
  user by name.
- Spike the trigger (5.2): that an `IStartupFilter` registered from the module's own extension method
  runs under `WebApplication` in `Program.cs`, that its middleware sees the query string of a
  Subsonic request without breaking the body buffering at `Program.cs:725`, and that a background
  run started from it does not block the request. If not, use the one-line fallback beside
  `EnsureAsync` in `GetPlaylists` (a hotspot claim first).
- Confirm that Navidrome accepts the playlist calls with the credential a run holds, including the
  explicit non-public flag on create and the `public` and `owner` fields on read-back.
- Confirm Navidrome's playlist APIs behave as the existing workers assume when a track list is
  long (the helpers page at 500, `NavidromePlaylistApi.cs:200`).
- Decide how long a run may last and how many may run at once, so a Monday morning does not start
  every user's run in the same second.
- Decide the playlist cover and description wording.

## 10. Conditions for the build (Security, FLA-15 sign-off)

Security approved the design with 12 conditions, binding on FLA-23 and checked in its PR review.
Source: the `[Security agent] Approved (design).` comment on FLA-15
(https://linear.app/cv-agent/issue/FLA-15/research-discover-weekly-as-a-separate-module). They were
written for the first design, which held a sign-in token for up to 24 hours. The human decided
(D7, 2026-10-10) that no credential is held, so each condition is re-checked below as **applies**,
**moot** or **changed**. Security still needs to confirm the changed and moot rows when the FLA-23
PR comes back to it (condition 12).

| # | Condition (short) | Status | Under the live-request design |
|---|---|---|---|
| 1 | Credential holding (R1.2, R1.5): `t`/`s` only, memory only, never logged or serialized, expire at point of use, 24 h limit, wipe on off/opt-out/sign-out, cap the number held | **Changed** | Nothing is held between requests, so the 24 h limit, the wipe-on-sign-out and the cap on held credentials are **moot**. What still **applies**: the credential lives only in the run's own variables, is converted with `WithoutPassword()` at hand-off so a password is not kept even for the run, is never written to a file, log, exception text, notification, dashboard answer or job record, never serialized, and is logged only through `ToString()`. The run is cancelled, and the credential dropped, when the switch goes off or the user opts out |
| 2 | Only opted-in users held; dashboard says a token is kept up to a day; API-key credentials skipped | **Changed** | The dashboard text becomes "Octo builds your list when you open a client and keeps no sign-in". When the module is on, every allowed user is covered unless they opt out (D8). **Applies:** an API-key sign-in has no `u`, so it is skipped in v1 (5.2); such users get no list and the status says why |
| 3 | Capture only after Navidrome accepts, user from the verified request, Subsonic paths only, prefer a filter to a global `IStartupFilter`, claim the hotspot if the fallback is used, credential used only for its own user, never returned | **Applies, rewritten as the trigger rule** | Same rules, now for starting a run instead of storing a hold (5.2): start only after `CredentialCheck` says Accepted, user name from `u` of that same request, only `/rest/` paths, one run per user at a time, the credential used for that user only. "A later request must not replace a good hold" is **moot** (no hold) |
| 4 | Resolve the 4.2 / 5.4 contradiction: no admin add after expiry unless cleared in the same run; documented residual risk if admin fill stays | **Moot** | There is no admin fill at all and no gap between clearing and writing: every write is made as the user in a run that has just read both sets (4.2 layer 3, 5.4). A song that lands later waits for the user's next live request. The residual risk is gone |
| 5 | Playlist privacy (R2.5, R2.8): user credential, explicit non-public, read back, fail closed, never another user's picks, id from the store | **Applies** | Unchanged (4.4, 5.4) |
| 6 | Isolation test (R2.7): A builds, B cannot reach A's playlist id, ledger, picks or credential; no cross-user store method reachable from a controller; B's exclusion set never has A's private playlists | **Applies** | Unchanged except that there is no stored credential to test. A new test asserts no module type has a field of type `SubsonicCredential` that outlives a run (4.2 tests) |
| 7 | ListenBrainz mapping: `validate-token` proof preferred; admin-only map under `/api/admin`; shows the mapping; validated; public data only; tokens masked | **Applies** | Unchanged (5.1) |
| 8 | External input (R4.1, R4.6, R4.7, R4.12): fixed hosts, no off-host redirects, timeout and size cap, MBID/ISRC validation, length and count caps, titles are data | **Applies** | Unchanged (7) |
| 9 | Abuse caps: per-user cap, server-wide weekly cap, per-hour rate, disk floor, size and duration limits (R4.8), no download while the exclusion set is unread, no file deletion | **Changed** | The per-user cap is now **10 songs a week** (D8), not 20. The rest **applies** (4.3) |
| 10 | Stored data (R1.4 spirit, R2.2): mode 0600, temp file then move, keyed by user, bounded, removed on opt-out, no tokens, counts only in logs | **Applies** | Unchanged (5.3). With no held credential, "no tokens" is also true by construction |
| 11 | Ships off, documented in `README.md` and `.env.example`; off means no calls, no holds, no files; section 8 decisions recorded on the PR | **Applies** | Unchanged; "no holds" is now trivially true. D7 and D8 are recorded on FLA-15 and are to be quoted on the FLA-23 PR |
| 12 | The FLA-23 PR returns to Security (auth, credentials, external input) | **Applies** | Unchanged. The credential-handling and trigger code in particular must be reviewed |
