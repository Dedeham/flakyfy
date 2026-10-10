# Plan for Wave 2 and the final checks

The detailed plan that FLA-17 asks for. It pulls the architecture map, the module conventions,
the security requirements, the research and design documents and the human decisions of
2026-10-10 into one place: what to build, in what order, and what is still open.

**Status: approved by the human on 2026-10-10 ("plan freigeben", recorded on FLA-17), in the
simplified form below.** The first draft (PR #12) was larger; the human asked for everything to be
simplified against the project brief, and this version is that simplification. Step 3 of FLA-17
(each Wave 2 issue carries its final scope) links each issue to its section here. Step 4 (a fresh
Architect reviewed this plan against the code) is done; its 24 items are folded in and listed in
section 6. One item needs a human decision (section 4, question 3).

Sources, all on `main`: `docs/architecture.md` (FLA-7), `docs/modules.md` (FLA-8),
`docs/security-requirements.md` (FLA-9), `docs/testing.md` (FLA-12), `docs/research/search.md`
(FLA-13), `docs/research/ytdlp-fallback.md` (FLA-14), `docs/research/discover-weekly.md`
(FLA-15), `docs/research/retention.md` (FLA-16).

---

## Summary

**Build order. At most two builds run at the same time.**

| Phase | Issues | What the user gets |
|---|---|---|
| 1 | FLA-19 | Imports keep order; liked songs become Navidrome favorites for that user only; playlists stay separate and private |
| 1 | FLA-21 with FLA-31 (one branch) | yt-dlp as last resort after 3 counted failed runs of the chain (6 hours apart), saved as MP3, outages waited out and the YouTube step, if enabled, left alone; default on once the yt-dlp trust question is decided |
| 2 | FLA-22 | Search: library results on top, outside results below, Deezer added, nothing outside can empty or break the library part |
| 2 | FLA-23 | Discover Weekly: a small module of our own, modelled on Made for you; one private playlist per user, all users with opt-out, 10 downloads per user per week; waits for FLA-19 and FLA-21 |
| 3 | FLA-20 | Two settings: may imports fetch songs, do liked songs become favorites (no new module) |
| 3 | FLA-25 | One missing assertion in the existing updater test |
| 3 | FLA-18 | YouTube Music import as Takeout CSV column aliases (and maybe public links); low priority, may be dropped |
| 4 | FLA-26, then FLA-27 | Regression with modules on and off, scans in CI, final security review |
| after FLA-28 merges | FLA-11 | The new cover PNG (both edit `CoverArtService.cs`) |
| postponed | FLA-24 | Retention cleanup: not built, the design stays as the record |

Phase 3 also respects the limit of two: FLA-20 and FLA-25 first, FLA-18 when one is merged.

**Every new module** is one folder under `octo/Services/<Module>`, one switch with its default in
four places, one registration line in `Program.cs`, its own tests, and a validation workflow in the
integration harness. Off means off. A module that touches auth, user data, credentials, external
input or files is reviewed by Security.

**Still open for the human: three questions** (section 4), one of them pending a decision (yt-dlp: freeze or verify).

---

## 1. Decisions already made by the human

All recorded on the issue named, on 2026-10-10, relayed by the orchestrator. Under the rule the
human set that day, a quoted relay is written approval for the action and scope it names.

| # | Decision | Recorded on |
|---|---|---|
| D1 | **Plan approved** in the simplified form of this document. | FLA-17 |
| D2 | **Retention build FLA-24 is postponed** ("Lohnt sich das Feature überhaupt? Dann lassen wir das mal weg."). Stays in Backlog, nobody starts it until a human lifts it. The design (`docs/research/retention.md`, with Security's 19 conditions) is the record. Ratings protect a song, an album or artist heart protects all its songs, Navidrome is not pinned. | FLA-16, FLA-24 |
| D3 | **Search (FLA-22).** Library results are always on top, outside results below. Robustness first. Latency must not be worse than today (2 to 4 seconds in the app). A failing outside source never breaks or empties the library results. Deezer is added as a second outside source. | FLA-22 |
| D4 | **Discover Weekly (FLA-23).** Our own small module, no third-party project integrated. Every user gets a list, with an opt-out. 10 auto-downloads per user per week. **No server-wide caps.** No credential is held: the list is built during the user's first live request of the week. A starred artist does not exclude all of that artist's songs. | FLA-15, FLA-23 |
| D5 | **yt-dlp fallback (FLA-21), lean v1.** Attempt counting (3 attempts, at least 6 hours apart), the fallback itself (MP3), hardening of what goes into the shim, yt-dlp pinned and checksum-verified, then **default on**. Outages of Soulseek and Lidarr are waited out and do not count as failed attempts. The extra conditions from the later Security rounds move to "after v1" unless needed for safety (section 2, FLA-21). | FLA-14, FLA-21 |
| D6 | **Import fidelity (FLA-19) comes first.** Liked songs become Navidrome favorites (the same set) and are **on by default**. Playlists are created separately and are private. A requested song plays right away (streamed) and is downloaded in the background so it is local next time. | FLA-19 |
| D7 | **YouTube Music import (FLA-18) is low priority** ("erstmal egal, viel wichtiger ist, dass der pro-User-Import richtig funktioniert"). | FLA-18 |
| D8 | **ImageSharp (FLA-28) stays on 3.1.12** with the cover size cap only. The remaining High advisories are accepted as a temporary risk by the human; the 4.x approval is withdrawn (4.x needs a Six Labors licence key in every non-Debug build). FLA-32 evaluates replacing ImageSharp with SkiaSharp. | FLA-28, FLA-32 |
| D9 | **The test-only stub container for the harness is approved** (one small container in `tests/integration/docker-compose.yml`, never in the root compose file or the images). | FLA-17 |
| D10 | **No new Medium security issues** are filed now. FLA-27 reports on the remaining audit findings. | FLA-17 |
| D11 | **GitHub Actions runs CI only**; `docker.yml` and `release-links.yml` are disabled. | memory file |
| D12 | **The orchestrator may merge verified PRs.** Tags, releases, deployments and running the updater against a real server still need a human each time. | memory file, PR #15 |
| D13 | All agent work runs on Sonnet. One commit per finished issue. | memory file |
| D14 | After the fresh review, the orchestrator decided (the human asked for the simplest version that meets the brief): reuse `StarOnArrival` for favorites; one shared verified-request observer (owned by FLA-19, used by FLA-23); FLA-23 makes its playlist through the admin path; FLA-31 shrinks to two lines of the existing hold; FLA-18, FLA-20 and FLA-25 shrink (sections below). The yt-dlp trust question is **not** decided. | FLA-17 |

Already built or in flight and not part of this plan: FLA-12 (harness, merged), FLA-29 (slskd
default login, merged), FLA-28 and FLA-30 (cover size cap, quarantine real path), FLA-10 (layout
jump, skipped by the human, left in Todo), FLA-11 (new cover PNG, waits for FLA-28).

---

## 2. One section per Wave 2 build issue

Words used. "Switch" is the on/off setting; each switch states one default in `.env.example`,
`docker-compose.yml`, `octo/appsettings.json` and the settings class (`docs/modules.md`).
"Common hotspots" are `octo/Program.cs` (one registration line), `.env.example`,
`docker-compose.yml`, `octo/appsettings.json` and `README.md` (one paragraph); each module changes
only its own lines in them and claims them first (`AGENTS.md`). "Harness" is `tests/integration/`
(`docs/testing.md`): each module adds seed data, at least one `test_<module>.py`, an A/B isolation
check for anything stored per user, and its routes in the shared stub (D9).

**The shared stub.** One small Python container (standard library only) in
`tests/integration/docker-compose.yml`, reachable only inside the test stack. It stands in for
Deezer, Last.fm, slskd, Lidarr and the yt-dlp shim, with canned answers per module. The first
builder who needs it (FLA-21) creates it; later modules add their routes in their own files under
`tests/integration/stubs/`, so they do not edit each other's. Deezer, Last.fm and ListenBrainz base
URLs are constants today, so the stub can stand in for them only after small base-URL settings exist:
FLA-22 adds Deezer's and Last.fm's (first commit), FLA-23 adds ListenBrainz's.

### FLA-19 Import fidelity: order, favorites, private playlists (phase 1)

- **Scope.** Audit and fix three behaviours. Order (A6) and privacy (A8) already exist in
  `ImportFileReader.ReadCsv`, `ImportPlaylists.SyncAsync` and `NavidromePlaylistApi`; favorites (A7)
  are missing (the liked list is only a playlist named "Liked Songs"). Write the three harness tests
  first. The order and privacy tests may pass on today's code and then act as locks; say so in the
  PR. Only favorites, and any real defect the order test finds after songs arrive later, can fail
  first.
- **Favorites, on by default (D6). Reuse `StarOnArrival`.** `StarOnArrival` already stars a song as
  the user with that user's own sign-in, token only and never the password (`credential.WithoutPassword()`),
  through `StarSongAsync`, which checks `getSong` and then calls `rest/star`. Its hold lives in memory
  for about a day, too short for a trickle of 20 songs an hour. So: liked songs go into a per-user
  **pending list of song ids and nothing else**, kept in the import store; whenever that user makes a
  verified live request, the observer below calls the existing `StarSongAsync` path for the ids that
  are in the library now. No second star path, no stored credential (the credential is a local
  variable of the call). The liked set and the playlist stay separate.
- **The shared observer (owned here, used by FLA-23).** There is no MVC filter infrastructure
  (`Program.cs` uses plain `AddControllers()`), so no filter and no spike. Two places already hold a
  verified request: `SubSonicController.cs` inside `GetPlaylists` after Navidrome accepted the sign-in
  (line ~381, next to the `EnsureAsync` fire-and-forget) and `ImportsSubsonicController.CallerAsync`.
  FLA-19 adds **one small interface** (working name `IVerifiedRequestObserver`, in
  `octo/Services/Common/`) called from those two places, fire and forget, passing the verified
  credential as a local value that observers must not keep. FLA-19 registers its stars observer;
  FLA-23 registers its own. The one-line edit in `SubSonicController.cs` is claimed.
- **Module.** Not a new module: fixes in `octo/Services/Imports` (existing code). New code in its own
  file `octo/Services/Imports/ImportStarsOnRequest.cs`.
- **Switch.** Imports already use the `IMPORTS_*` prefix and the `Imports` settings section, so:
  `IMPORTS_STAR_LIKED` -> `Imports__StarLiked` in `ImportSettings`, default **true**, in the four
  places. The dashboard settings API hand-lists `Imports:*` keys in `AdminController.cs`; FLA-19
  does **not** edit that file (`docs/modules.md`), so the switch is an environment setting. FLA-20
  decides whether it appears in the dashboard.
- **Hotspots.** Common hotspots for the switch, plus the one claimed line in `SubSonicController.cs`.
  Shared, not hotspots: `ImportService.cs`, `ImportPlaylists.cs`, `ImportStore.cs`,
  `ImportsSubsonicController.cs`, `StarOnArrival.cs` (reused, not changed unless a public method is
  needed).
- **Harness.** `test_import_fidelity.py`: (1) CSV of 6 rows, some songs present, others arriving
  later; playlist order equals source order before and after; (2) liked songs are starred for the
  importer and for nobody else (alice and bob overlap on songs, so a leak shows); (3) the playlist
  is non-public and owned by the importer, checked in Navidrome directly and through Octo as bob.
- **Security inherited.** R2.1 to R2.5, R2.7, R1.2 (the live credential is never logged, stored or
  kept by an observer). Security reviews the PR (user data, credentials).
- **Depends on.** FLA-12 (done). Blocks FLA-23 (the observer) and FLA-20.

### FLA-21 Build the yt-dlp fallback, with FLA-31 (phase 1)

- **How it relates to the chain.** The coordinator already walks `EffectiveHeartDownloadSources()`,
  and one of the steps is `HeartDownloadSource.YouTube`. **If the YouTube step is enabled in the
  user's chain, the fallback does nothing** (it was already tried in its place); this is tested. The
  **only new behaviour** is: with the YouTube step off, a song whose whole chain has failed 3 times
  (counted chain runs, at least 6 hours apart, outages not counted) is fetched with yt-dlp as MP3.
  A "counted failed attempt" is one full run of the enabled chain that ended in not found or in an
  error; Lidarr failing counts as a failed attempt (D5), a Soulseek outage does not.
- **Scope (lean v1).**
  1. *Shim input hardening* (closes SEC-8): video id `^[A-Za-z0-9_-]{11}$` at the shim edge and in
     `YouTubeResolver`; `dest` confined to `.octo-incoming/` with `%` escaped; `--` before the URL;
     free text only behind the fixed `ytsearch1:`/`ytsearch5:` prefix; text cleaned and length-capped
     at one choke point in `YouTubeResolver`; the shim enforces duration, file size and timeouts
     itself; no raw query in logs. If a new shim file is added, `yt-dlp-shim/Dockerfile` copies
     `app.py gate.py ytm.py` by name, so its `COPY` line changes (nothing in CI catches a miss).
  2. *yt-dlp trust* (Security condition 11): **pending a human decision**, two options in section 4
     (question 3). Until it is decided and built, the switch ships `false`.
  3. *Outage handling* (FLA-31, below).
  4. *The module*: a small extension point (`IAcquisitionFallback`) at the end of
     `HeartAcquisitionCoordinator.AcquireTrackAsync`, counting, the download through the existing
     YouTube path as MP3, a YouTube-sourced tag. It applies to **hearts and the import trickle only**
     (both call `AcquireTrackAsync`); not album hearts, not play.
  5. Gives up after 72 hours with a plain dashboard line (counts, no song list).
- **Settings in v1: only** `Enabled`, `FailedAttempts` (3, minimum 2), `AttemptSpacingHours` (6,
  minimum 1) and `MaxWaitHours` (72, never below `FailedAttempts x AttemptSpacingHours + 6`,
  computed with 64-bit arithmetic; the computed minimum wins). Per-hour limits, the maximum video
  length and the free-space floor are constants in code (12 downloads an hour, 15 minutes, 5 GB),
  not settings.
- **Two songs sources, two counters (answers how this meets the trickle).** Heart songs are counted
  in the module's own store (`ytdlp-fallback.json`, ids, user, counts and times, `0600`). **Import
  trickle songs reuse `TrickleJob.Attempts` and its state**; the new store does not hold them. Today
  a not-found trickle song is retried after `RetryNotFoundAfter` = 14 days
  (`TrickleQueue.cs:15`) at the per-hour cap `SongsPerHour`. With the module on, a not-found trickle
  song is retried after `AttemptSpacingHours` instead, until `Attempts` reaches `FailedAttempts`;
  then the fallback runs once; if that fails the song falls back to the 14-day rule. The 72 hour
  give-up applies to hearts; for trickle songs the attempt count is the limit. With the module off
  nothing changes.
- **Moved to "after v1"**: the persisted given-up list with its own retention and caps, moved-aside
  corrupt store files, deleting a user's records with the user, a rich admin view,
  `RetryBetterDays` and the "YouTube copy" label, and the other settings. **Kept for safety**: strict
  store entries (a missing or corrupt field never counts as "enough attempts"; the store fails toward
  not fetching), a re-heart never resets the count, "enabled but not set up" counts as unavailable,
  tags that carry no user identity, a `0600` store, an A/B isolation test, the free-space check.
  Security confirms the cut on the PR (the split is the Architect's reading).
- **Discover Weekly songs are excluded by construction**: the fallback is offered only from
  `AcquireTrackAsync`, which only hearts and the trickle call. FLA-23 must not use it (section FLA-23),
  and a harness test shows a Discover Weekly fetch is never offered to the fallback.
- **Module.** `octo/Services/YtDlpFallback/`, tests `octo.Tests/YtDlpFallback/`.
- **Switch.** `YTDLP_FALLBACK_ENABLED`, default **false** until item 2 is decided and built, then
  **true** (D5).
- **Stub and base URLs.** This issue creates the shared test stub (below). Slskd, Lidarr and the shim
  URLs are already settings.
- **Hotspots.** Common hotspots, plus `.github/workflows/ci.yml` (own `pip-audit` job). Shared, not
  hotspots: `HeartAcquisitionCoordinator.cs`, `TrickleWorker.cs`, `TrickleQueue.cs`,
  `YouTubeResolver.cs`, `LocalLibraryService.cs`, `yt-dlp-shim/app.py`, `yt-dlp-shim/Dockerfile`.
- **Harness.** `test_ytdlp_fallback.py`: (1) a song missing from stubbed Soulseek and Lidarr arrives
  through the stubbed shim and carries the YouTube-sourced tag; (2) during a stubbed Soulseek outage
  nothing is taken from YouTube; (3) with the YouTube step enabled the fallback does nothing; (4) a
  Discover Weekly fetch is never offered to the fallback; (5) alice's parked songs are invisible to
  bob. Waiting 3 x 6 hours is not testable in a run: the test pre-seeds a complete valid store entry
  with past times, and unit tests use a fake clock. Say so in the PR.
- **Security inherited.** Conditions 1 to 6, 8, 10, 11, 13 and 19 to 21 of
  `docs/research/ytdlp-fallback.md` section 12. Security reviews the PR.
- **Depends on.** FLA-12 (done). Blocks FLA-23 (the exclusion test).

### FLA-31 Soulseek outage handling (inside FLA-21, phase 1)

- **Minimal fix.** The outage machinery already exists: `SoulseekHoldStore`,
  `ISoulseekLink.WaitForLoginAsync`, `HoldForSoulseekAsync` and `SoulseekDroppedAsync`. The gap is
  two places that skip the wait when Lidarr is later in the chain: `SoulseekDeadline`
  (`HeartAcquisitionCoordinator.cs` lines 358-363) and `TrickleWorker.SoulseekOut` (lines 88-96).
  Fix those two, using the existing hold machinery, with tests for hearts and for the trickle.
- **Not in scope** unless FLA-21 turns out to need it: a typed `NotFound`/`Unavailable` refactor
  across Soulseek, Lidarr and the YouTube resolver. The fallback gets its own "is Soulseek down?"
  check before counting (a `null` or `Unknown` link status counts as down), so it does not depend on
  `SoulseekLink` changing. The "Lidarr failure is not not-found" criterion is dropped from this issue:
  a Lidarr failure counts as a failed attempt in the fallback (D5), and the chain's own behaviour
  stays as it is.
- Same branch and PR series as FLA-21. If FLA-21 is delayed the fix can merge alone.

### FLA-22 Improve search results (phase 2)

- **Scope.** Library results stay on top. The outside half gets Deezer track search as a second
  source next to Last.fm, merged by reciprocal rank with the existing `SongIdentity.MatchKey` dedupe,
  and it works with no Last.fm key. One small Last.fm fix: `autocorrect=1` on the artist pad. Out of
  scope: type-ahead (`songCount` under 12 keeps skipping outside calls), library-side widening,
  YouTube Music as a source, a Last.fm measurement.
- **Robustness (D3), concretely.** Today `SubSonicController.cs` waits for the outside build
  (`await externalTask`, line ~1133) up to the 10 second `BuildTimeout`, so a slow outside source
  delays the library rows, and `SearchPlaylistsAsync` (~1146) has no try/catch, unlike the album and
  artist tasks. Therefore: (a) the outside half gets a **short response deadline, default 3 seconds**
  (`SearchFusion:OutsideDeadlineSeconds`), after which the library rows are returned alone and the
  build keeps running in the background for the cache; (b) a try/catch around `SearchPlaylistsAsync`.
  Both are small edits in `SubSonicController.cs`, claimed. Tests cover a Deezer error, a Last.fm
  error, both failing, and a slow source.
- **The hook is wider than one branch.** The key gate is in `ExternalSearchService.GetAsync` (line 87:
  no Last.fm key returns empty), so "works without a key" changes that line; the constructor takes
  `LastFmService? lastFm`, which changes too; and `BuildAsync` gets the fusion branch. Callers in the
  controller (lines ~1099, ~1238, ~4285) need no change.
- **Base URLs (harness).** Deezer, Last.fm and ListenBrainz base URLs are constants today
  (`DeezerMetadataService.cs`, `DeezerRateLimitHandler.cs`, `DeezerCoverArtLookup.cs`,
  `LastFmService.cs`, `ListenBrainzService.cs`). This issue adds **configurable base URLs for Deezer
  and Last.fm** as its first commit (and makes the rate-limit handler key off the configured host);
  FLA-23 adds the ListenBrainz one. The harness compose currently sets
  `Subsonic__EnableSearchDiscovery: "false"` and an empty `LastFm__ApiKey`; the FLA-22 test flips both.
- **Module.** `octo/Services/SearchFusion/` with `ISearchSongSource`, a Last.fm source, a Deezer source
  (through the existing Deezer HTTP client so `DeezerRateLimiter` applies) and the fusion function.
- **Switch.** `SEARCH_FUSION_ENABLED`, default **true** (Deezer added, D3). Off runs today's path.
- **Hotspots.** Common hotspots, plus `SubSonicController.cs` (the two small edits above).
  Shared, not hotspots: `ExternalSearchService.cs`, `LastFmService.cs`, the Deezer files above.
- **Acceptance numbers.** Fixture `docs/research/search-queries.json` with canned source answers: at
  least 12 of the 13 scored cases meet `minSongs` and `mustIncludeInTop` (the one allowed miss is
  `Mr. Brightside live`), type-ahead makes zero outside calls, "wekend" is best effort, both
  scenarios pass. Latency no worse than today: with the 3 second deadline the library response is
  bounded; report the before and after numbers (reference: 2 to 4 seconds in the app).
- **Harness.** `test_search_fusion.py` against the stub, plus an outside-failure and a slow-source
  test. No per-user data.
- **Security inherited.** R4.12 (fixed hosts; the configurable base URL is a setting an admin sets,
  never a request value), timeouts and size caps, R4.6. Security reviews only if a new host or URL
  setting is added; the base-URL settings count, so tell Security.
- **Depends on.** FLA-12 (done).

### FLA-23 Build the Discover Weekly module (phase 2)

- **Model it on Made for you.** `GeneratedPlaylistService.ListAsync(username, auth)` already builds
  per-user lists on a live request, with a case-folded `UserKey`, a per-user refresh deadline, retry
  after failure and a first-list wait. Reuse that pattern (not its code: Made for you is not edited).
  `NewReleasesBuilder` already mints outside songs from Deezer through `ExternalIdRegistry`; reuse
  `ExternalIdRegistry` the same way for ListenBrainz picks. Reuse `ListenBrainzService.TokenFor(username)`
  and `ValidateTokenAsync` to prove a user's ListenBrainz name from their own stored token.
- **How it differs from Deep Cuts and the other Made for you lists.** Deep Cuts and Rediscover pick
  songs **already in the library** that the user has not played or has loved; New Releases follows
  the user's top artists. Discover Weekly is for **new songs the user does not have**: every pick is
  checked against all of that user's playlists and favorites, and most picks are fetched. So it has
  no "library songs the user never touched" source (that would duplicate Deep Cuts).
- **Scope (lean).** Candidates come from **ListenBrainz only** (the only outside source): its
  recommendations for users whose name is proved by their own token, and ListenBrainz similarity
  seeded from the user's own favorites (public data, library-derived seeds, no user name sent), so
  every user can get a list. No Last.fm, no admin-typed name map, no library-pick source in v1. A
  fail-closed exclusion gate against the user's playlists and favorites (`docs/research/discover-weekly.md`
  section 4.2). **Every allowed user is covered unless they opt out** (D4). Up to 10 outside songs
  per user per week; no server-wide caps (D4).
- **Credential use (orchestrator decision).** The private playlist is created and updated **through
  the existing admin path** (`NavidromePlaylistApi`, as imports do: created non-public, handed to the
  user, read back and checked non-public). The user's live credential is used **only to read that
  user's favorites** for the exclusion gate, as a local variable, never stored. This differs from
  Security's earlier condition that the playlist be created with the user's own credential;
  **Security must confirm it in the PR review** (it reduces the credential surface, but it is an
  admin write into a user's playlist, R2.4).
- **The trigger.** FLA-23 registers an observer on the shared interface FLA-19 builds (no filter, no
  spike): it runs only for a verified request with `u` and `t`/`s` or `p`, never an API key
  (API-key sign-ins get no list; the status says why), one case-folded user key, one run per user at a
  time. Blocked by FLA-19.
- **Fetching and marking.** Picks are fetched by enqueuing on `TrackAcquisitionQueue` /
  the acquisition worker, **not** through `HeartAcquisitionCoordinator.AcquireTrackAsync`. That is
  what keeps them out of the yt-dlp fallback (FLA-21 section: excluded by construction); the PR
  verifies the entry point it uses and a harness test shows no fallback offer. Blocked by FLA-21 for
  that test.
- **Opt-out.** A small panel for any signed-in user. The controller routes use `[AdminOpen]` (as
  `ImportsController` does) so non-admin users reach them, and every non-GET needs the `X-Octo-Admin`
  header. Opting out stops runs and deletes their rows. Panel in its own JS file.
- **Module.** `octo/Services/DiscoverWeekly/`, `octo/Controllers/DiscoverWeeklyController.cs`.
  Made for you is not edited.
- **Switch.** `DISCOVER_WEEKLY_ENABLED`, default **false** until merged and checked;
  `DISCOVER_WEEKLY_PER_USER_WEEKLY` default 10.
- **Base URL.** Adds the configurable ListenBrainz base URL for the stub.
- **Hotspots.** Common hotspots. No `SubSonicController.cs` edit (the observer line is FLA-19's).
- **Harness.** `test_discover_weekly.py`: alice and bob (overlapping private playlists and favorites)
  each make a request and each get a private playlist; **no song in a user's list is in any of that
  user's playlists or favorites**; bob cannot read, change or delete alice's list (R2.7), through the
  `[AdminOpen]` routes with the guard header; "Alice" and "alice" share one cap; an API-key request
  starts nothing; opt-out stops runs; switch off makes no run and writes no file; a pick is never
  offered to the yt-dlp fallback. ListenBrainz comes from the stub.
- **Security inherited.** `docs/research/discover-weekly.md` section 10: 1 to 3, 5 (see the admin
  path note above), 6, 8, 10 to 18. Conditions 7 and 9 no longer apply. PR returns to Security.
- **Depends on.** FLA-12 (done), **FLA-19** (observer), **FLA-21** (exclusion test). Declared as
  Linear blockers.

### FLA-20 Admin setting: what imports are used for (phase 3)

- **Scope (simplified: two booleans, no policy module).** Today fetching is a **per-list user choice**:
  `ImportList.KeepPlaylist` and `GetMissing` default to false and are set by `SetPlaylistAsync` and
  `SetGetMissing`. So the admin setting is a **ceiling**: `Imports__AllowFetch`
  (`IMPORTS_ALLOW_FETCH`, default **true**, meaning the user's per-list choice applies) and the
  `Imports__StarLiked` switch from FLA-19. When `AllowFetch` is false, `SetGetMissing` refuses and the
  fetch loop (`ImportService.cs` ~677) skips lists; lists already set to fetch keep their flag, so
  turning the ceiling back on resumes them. "Playlist only" is both booleans off. No "taste seed"
  option (Discover Weekly does not read imports).
- **Dashboard.** A small block in the existing imports settings area with the two checkboxes. This
  needs the keys added to the hand-written `Imports:*` list in `AdminController.cs` (~2261): a claimed
  shared edit, named in the PR.
- **Module / switch.** No new module and no new switch: two properties of `ImportSettings`.
- **Hotspots.** Common hotspots (the `AllowFetch` lines), `AdminController.cs`, `admin.js` /
  `index.html` (one marked block).
- **Harness.** `test_import_policy.py`: with `AllowFetch` off nothing is queued, with it on a fetch list
  queues, the setting survives an Octo restart, a non-admin cannot change it. Browser check at desktop
  and phone width.
- **Security inherited.** R3.1 to R3.3, R3.5, R3.6; booleans validated server-side.
- **Depends on.** FLA-19.

### FLA-25 Verify updates on production installs (phase 3, tiny)

- **Scope.** Most of it is already tested: `scripts/updater/test-updater.sh` covers "a failed build
  changes nothing", "a release that will not stay up is rolled back", "a failed rollback says so" and
  untracked `.env`; CI runs it and `UpdateHostTests` covers the Octo side. The one missing assertion
  is that `docker-compose.override.yml` and `config/` survive an update. **Add that assertion to
  `scripts/updater/test-updater.sh`** and cite the existing tests in the PR. No disposable-stack
  workflow. Nothing pushes images or tags; the updater is never run against a real server.
- **Module / switch.** None. **Hotspots.** None expected.
- **Security inherited.** R3.8 (no new ports); the no-Docker-socket design stays.
- **Depends on.** FLA-12 (done).

### FLA-18 YouTube Music import, per user (phase 3, low priority)

- **Scope (simplified, not a module).** `ImportFileReader.ReadCsv` already reads any CSV with title
  and artist columns and several playlists per file, and keeps the order. Google Takeout's YouTube
  Music CSV most likely needs only **column aliases** in that reader and a test. **The Takeout header
  names are to be verified against a real export before the aliases are written; the reviewer
  believes they differ from the names currently listed (`ImportFileReader.cs` ~149-151).** Public
  playlist links use the existing `Link` source pattern (`AddLinkAsync`): a small reader for YouTube
  playlist URLs (host built by Octo, id validated), reading through a shim route; if that route is
  more than small, drop the link path and ship CSV only. No new controller, folder, store or queue.
  No cookies, tokens or headers stored. Liked songs follow the FLA-19 rule.
- **Switch.** CSV aliases are plain parsing and need none. The link path is behind
  `IMPORTS_YOUTUBE_LINKS` (`Imports__YouTubeLinks`), default **false**.
- **Hotspots.** Common hotspots for the switch (only if the link path is built); shared, not hotspots:
  `ImportFileReader.cs`, `ImportService.cs`, and `yt-dlp-shim/app.py`/`ytm.py` with the shim
  `Dockerfile` `COPY` line if a shim file is added.
- **Harness.** `test_ytm_import.py`: alice and bob each import a generated Takeout-shaped CSV; each
  sees only their own list; order kept; bob cannot read or delete alice's list by id.
- **Security inherited.** R1.5 (nothing stored), R2.1 to R2.3, R2.7, R4.12, R4.13, R4.1, R4.6.
  Security approves the PR if the link path is built.
- **Depends on.** FLA-19 (same code), FLA-20 (shared settings), FLA-21 only if the link path adds a
  shim route (its hardening comes first).

### FLA-24 Build the 30-day retention cleanup (POSTPONED)

Postponed by the human (D2). Stays in Backlog; no agent starts it until a human lifts it in writing.
If lifted, the design and Security's 19 conditions are in `docs/research/retention.md` sections 9 to
11, the credential option (A recommended) and the snapshot trigger place must be decided, and going
live (`RETENTION_DRY_RUN=false`) needs written approval. FLA-26 and FLA-27 do not wait for it.

### FLA-26 Full regression with modules on and off, and security scans in CI (phase 4)

- **Scope.** Matrix: the harness with every new module on, and with every new module off. Dependency
  scans for .NET, Rust and Python plus a container image scan; High or Critical fails the build
  (`docs/security-requirements.md` R6.3, R6.4). The Python scan for the shim already exists from
  FLA-21; this issue adds the rest and the matrix. Existing CI jobs are unchanged apart from additions.
  Flaky tests are fixed or filed. **FLA-10 is no longer a blocker** (skipped by the human; the Linear
  relation is removed). The remaining High advisories on ImageSharp 3.1.12 are an accepted risk (D8),
  so the .NET scan records them as a known exception on the PR until FLA-32 decides.
- **Switch / module.** None.
- **Hotspots.** `.github/workflows/*` (new jobs or files).
- **Depends on.** All Wave 2 builds except FLA-24.

### FLA-27 Final security review of all new modules (phase 4)

- **Scope.** One pass against `docs/security-requirements.md`, marked pass or fail, with a status for
  every FLA-9 finding (SEC-1 is the accepted risk of D8; SEC-2 is closed by FLA-29; SEC-3 to SEC-13
  get a status each and are reported, not filed as new Medium issues, D10). The items moved to "after
  v1" in FLA-21 are listed as residual risks with their conditions. Retention is out of scope while
  FLA-24 is postponed.
- **Output.** A Linear document on the project; no release approval.
- **Depends on.** All Wave 2 builds except FLA-24.

---

## 3. Build order and overlaps

At most two builds at the same time:

1. **Phase 1:** FLA-19 and FLA-21 with FLA-31.
2. **Phase 2:** FLA-22 and FLA-23. FLA-23 is blocked in Linear by FLA-19 (the observer) and FLA-21
   (the exclusion test), so it cannot start earlier.
3. **Phase 3:** FLA-20 and FLA-25, then FLA-18. FLA-20 needs FLA-19; FLA-18 is blocked by FLA-19 and
   FLA-20 (and FLA-21 only if it adds a shim route).
4. **Phase 4:** FLA-26, then FLA-27.
5. FLA-11 any time after FLA-28 merges (nothing in this plan touches cover code).

| Overlap | Handling |
|---|---|
| Acquisition chain (`HeartAcquisitionCoordinator`, `TrickleWorker`) | FLA-21 and FLA-31 are one branch. |
| `octo/Services/Imports` | FLA-19, then FLA-20, then FLA-18. |
| The verified-request observer | FLA-19 builds the interface and its two call lines; FLA-23 only registers against it. |
| Shim and `YouTubeResolver` | FLA-21 first; FLA-18 goes after if it adds a shim route. |
| Shared stub and base URLs | FLA-21 creates the stub; FLA-22 adds Deezer and Last.fm base URLs; FLA-23 adds ListenBrainz's. |
| `SubSonicController.cs` | FLA-19 (observer line) and FLA-22 (two small edits) each claim only their lines; they are in different phases or serialised by the claim. |
| `AdminController.cs`, `admin.js` | FLA-20 only; FLA-23's panel uses its own JS file. |
| The five common hotspot files | One open claim per hotspot at a time, which already serialises most of phases 1 and 2. Merge in order; rebase on `main` before review; never drop someone's lines. |

Review rule per PR: QA verifies against its criteria; Security reviews FLA-19, FLA-21, FLA-23, FLA-18
(if the link path is built) and any change that adds a host, port, URL setting or admin route; the
orchestrator merges verified PRs (D12).

---

## 4. Open questions for the human

Three remain. Everything else is answered by the decisions in section 1.

1. **Deezer's terms.** *Recommend: one person reads Deezer's published terms for the public API
   before FLA-22 is merged.* Deezer is already used by default for albums and enrichment without a
   key, and FLA-22 adds one request per search of 12 songs or more. I did not read the terms. The
   build can proceed meanwhile; the switch is one setting if the answer is no.
2. **"A requested song plays right away and is downloaded in the background" (D6).** Octo can already
   stream an outside song from YouTube at the first byte and download it on play, but
   `DOWNLOAD_ON_PLAY` is off by default (`docs/architecture.md` section 3.2). *Recommend: confirm that
   you want it on by default.* If yes, it is a one-line default change with its README line, filed as
   a small separate Backend issue.
3. **PENDING HUMAN DECISION. How is yt-dlp made trustworthy for the fallback (Security condition 11)?**
   Your earlier answer was "pinned and checksum-verified, then default on". The review found that the
   shim updates yt-dlp itself (`-U` on a 403; `YTDLP_VERSION=latest` in the Dockerfile) because
   YouTube extractors break within weeks, and that a frozen binary would break **playback of every
   outside song**, not just the fallback. Two options; I am not deciding between them:
   - **A. Verify, don't freeze.** Keep the self-update, but check the installed binary and the one `-U`
     fetches against yt-dlp's published SHA2-256 checksums, and add `pip-audit` for the shim. Playback
     keeps working; the residual risk is a compromised upstream release that has valid checksums.
   - **B. Hard pin.** A fixed yt-dlp version, self-update off. Strongest against a bad upstream
     release; but someone must bump the pin (suggested: monthly or on a breakage report), and
     playback of outside songs may break between bumps. This accepts a playback risk explicitly.
   Until you decide, the FLA-21 switch ships off. Either option satisfies condition 11 once built;
   Security re-reviews when the default turns on.

A decision I took inside the plan that you may overrule: none left. (The earlier "fallback declines
Discover Weekly songs" rule is replaced by a structural one: Discover Weekly does not use the code
path the fallback hooks into.)

---

## 5. Risks, and what to cut

| Risk | Mitigation |
|---|---|
| The shared observer misses a request path | Only two places call it (`GetPlaylists`, `ImportsSubsonicController.CallerAsync`); a user who never opens a client that fetches playlists gets no run; accepted. Fallback: a dashboard "Build now" button (a dashboard sign-in is also a verified request) |
| FLA-21 touches the most shared code | Hardening and typed outcomes first, behaviour-preserving, each tested; off means off is tested; Security reviews the shim change before the module |
| Default-on for the yt-dlp fallback | Depends on the pending decision (question 3); the switch ships off until then; Security re-reviews when the default is on |
| The harness cannot test time | Fake clock in unit tests; the harness pre-seeds valid entries; each PR says so |
| Unverified upstream behaviour (Deezer terms, ListenBrainz `validate-token`, Navidrome API details) | Each is a build-time spike with a stated stop condition |
| Accepted High advisories on ImageSharp 3.1.12 (D8) | The size cap is in; FLA-32 evaluates a replacement; FLA-27 reports it |
| CI evidence | Docs-only PRs used `[skip ci]` and had no CI run to cite; a build PR must confirm `ci.yml` and `integration.yml` actually ran, and say so when they did not |
| Auto-downloading at volume | Per-user cap of 10 a week for Discover Weekly and the yt-dlp per-hour limits; each is a setting |

**What I would cut if time is short, in order:** FLA-18, then FLA-20, then FLA-25's tests beyond the
failed-update case. I would not cut FLA-19, FLA-21 with FLA-31, FLA-22, FLA-26 or FLA-27. If FLA-23's
observer approach proves unusable, ship it as the "Build now" button instead of dropping it.

---

## 6. Fresh review of this plan, and what became of it

The step-4 review (24 items, no blockers) was folded in on 2026-10-10. "Resolved" means the plan text
and the issues now say it; "pending human" means a person must decide.

| # | Item | Result |
|---|---|---|
| 1 | FLA-19 ignores `StarOnArrival` | Resolved: reused, ids only in the pending list (FLA-19) |
| 2 | Hook point | Resolved: one shared observer, no filter or spike (FLA-19, FLA-23); FLA-19 blocks FLA-23 |
| 3 | FLA-23 can create the playlist without the user's credential | Resolved: admin path, credential only for reading favorites; Security to confirm |
| 4 | Made for you is the model | Resolved: reuse list added, Deep Cuts difference stated, library-pick source dropped |
| 5 | YouTube step already in the chain | Resolved: the fallback does nothing when it is on; tested |
| 6 | FLA-31 gap is small | Resolved: two places only, existing hold machinery |
| 7 | Trickle retries | Resolved: trickle songs reuse `TrickleJob.Attempts`; retry spacing replaces the 14-day wait until the count is reached |
| 8 | Pin versus self-update | **Pending human** (section 4, question 3) |
| 9 | Harness base URLs | Resolved: explicit prerequisite commits in FLA-22 and FLA-23; compose flags noted |
| 10 | FLA-22 hook wider | Resolved: `GetAsync` gate, constructor and `BuildAsync` branch listed |
| 11 | Library delayed or emptied | Resolved: 3 second outside deadline, try/catch on the playlist task, controller lines claimed |
| 12 | Marking Discover Weekly songs | Resolved: no field; excluded by construction; tested |
| 13 | Shim `COPY` list | Resolved: in FLA-21 and FLA-18 |
| 14 | Non-admin routes | Resolved: `[AdminOpen]` and the guard header in FLA-23 |
| 15 | Setting names | Resolved: `IMPORTS_*`; no `AdminController` edit in FLA-19; FLA-20 owns it |
| 16 | FLA-18 dependencies | Resolved: FLA-19, FLA-20, FLA-21 if a shim route is added |
| 17 | Undeclared blockers | Resolved: FLA-19 and FLA-21 block FLA-23 in Linear |
| 18 | FLA-18 is not a module | Resolved: CSV aliases (header names to verify) and maybe links |
| 19 | FLA-25 mostly covered | Resolved: one assertion |
| 20 | FLA-20 text wrong | Resolved: two booleans, ceiling semantics stated |
| 21 | FLA-21 settings | Resolved: four settings, rest constants |
| 22 | FLA-23 size | Resolved in part: ListenBrainz kept as the only outside source, library-pick source dropped; Last.fm out |
| 23 | Status line and counting wording | Resolved |
| 24 | Hotspot serialisation | Resolved: wording in section 3 |
