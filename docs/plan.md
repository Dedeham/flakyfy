# Plan for Wave 2 and the final checks

The detailed plan that FLA-17 asks for. It pulls the architecture map, the module conventions,
the security requirements, the research and design documents and the human decisions of
2026-10-10 into one place: what to build, in what order, and what is still open.

**Status: approved by the human on 2026-10-10 ("plan freigeben", recorded on FLA-17), in the
simplified form below.** The first draft (PR #12) was larger; the human asked for everything to be
simplified against the project brief, and this version is that simplification. Step 3 of FLA-17
(each Wave 2 issue carries its final scope) links each issue to its section here. Step 4 (a fresh
Architect reviews this plan) is still to do.

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
| 1 | FLA-21 with FLA-31 (one branch) | yt-dlp as last resort after 3 failed Soulseek/Lidarr attempts, saved as MP3, outages waited out; yt-dlp pinned and verified; then on by default |
| 2 | FLA-22 | Search: library results on top, outside results below, Deezer added, nothing outside can empty or break the library part |
| 2 | FLA-23 | Discover Weekly: a small module of our own; one private playlist per user, all users with opt-out, 10 downloads per user per week |
| 3 | FLA-20 | Admin page: what each import source is used for |
| 3 | FLA-25 | A short verification of the existing update path |
| 3 | FLA-18 | YouTube Music import, low priority, may be dropped |
| 4 | FLA-26, then FLA-27 | Regression with modules on and off, scans in CI, final security review |
| after FLA-28 merges | FLA-11 | The new cover PNG (both edit `CoverArtService.cs`) |
| postponed | FLA-24 | Retention cleanup: not built, the design stays as the record |

Phase 3 also respects the limit of two: FLA-20 and FLA-25 first, FLA-18 when one is merged.

**Every new module** is one folder under `octo/Services/<Module>`, one switch with its default in
four places, one registration line in `Program.cs`, its own tests, and a validation workflow in the
integration harness. Off means off. A module that touches auth, user data, credentials, external
input or files is reviewed by Security.

**Still open for the human: two questions only** (section 4).

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
`tests/integration/stubs/`, so they do not edit each other's.

### FLA-19 Import fidelity: order, favorites, private playlists (phase 1)

- **Scope.** Audit and fix three behaviours. Order (A6) and privacy (A8) already exist in
  `ImportFileReader.ReadCsv`, `ImportPlaylists.SyncAsync` and `NavidromePlaylistApi`; favorites (A7)
  are missing (the liked list is only a playlist named "Liked Songs"). Write the three harness tests
  first. The order and privacy tests may pass on today's code and then act as locks; say so in the
  PR. Only favorites, and any real defect the order test finds after songs arrive later, can fail
  first.
- **Favorites, on by default (D6).** Octo never holds a password and cannot act as a user without a
  live credential (`docs/architecture.md`, finding 2; R2.5 forbids an admin shortcut). So liked
  songs go into a per-user "pending stars" list (song ids only) in the import store and are applied
  on that user's next live Subsonic request with that request's verified credential, never stored.
  Songs already in the library are starred at once; fetched songs as they land. The liked set and
  the playlist are separate: removing the playlist does not touch favorites.
- **Module.** Not a new module: fixes in `octo/Services/Imports` (existing code). New code in its
  own file `octo/Services/Imports/ImportStarsOnRequest.cs`.
- **Switch.** `IMPORT_STAR_LIKED`, default **true**.
- **Hotspots.** Common hotspots for the switch. Shared, not hotspots: `ImportService.cs`,
  `ImportPlaylists.cs`, `ImportStore.cs`. If the request hook cannot be registered from inside the
  imports code, one claimed line in `SubSonicController*.cs` (the same spike as FLA-23; whichever is
  built first proves the registration point).
- **Harness.** `test_import_fidelity.py`: (1) CSV of 6 rows, some songs present, others arriving
  later; playlist order equals source order before and after; (2) liked songs are starred for the
  importer and for nobody else (alice and bob overlap on songs, so a leak shows); (3) the playlist
  is non-public and owned by the importer, checked in Navidrome directly and through Octo as bob.
- **Security inherited.** R2.1 to R2.5, R2.7 (verified user, A/B test), R1.2 (the live credential is
  never logged or stored). Security reviews the PR (user data, credentials).
- **Depends on.** FLA-12 (done). No other Wave 2 issue.

### FLA-21 Build the yt-dlp fallback, with FLA-31 (phase 1, one branch)

- **Scope (lean v1).**
  1. *Shim input hardening* (closes SEC-8): video id `^[A-Za-z0-9_-]{11}$` at the shim edge and in
     `YouTubeResolver`; `dest` confined to `.octo-incoming/` with `%` escaped; `--` before the URL;
     free text only behind the fixed `ytsearch1:`/`ytsearch5:` prefix; text cleaned and length-capped
     at one choke point in `YouTubeResolver`; the shim enforces duration, file size and timeouts
     itself; no raw query in logs.
  2. *Pin yt-dlp and verify it* (Security condition 11): a pinned, checksum-verified yt-dlp in the shim
     image, with the runtime self-update (`-U`) off or limited to the pinned version, plus a
     `pip-audit` job for the shim in `ci.yml`. This is the precondition for default-on.
  3. *Typed source outcomes* (also **FLA-31**): `NotFound` versus `Unavailable` from Soulseek, Lidarr
     and the YouTube resolver. A Soulseek outage (logged out, crashed, unreachable) is waited out
     whatever Lidarr's position; a Lidarr error is not "not found".
  4. *The module*: counts failed attempts (default 3, at least 6 hours apart), then downloads through
     the existing YouTube path as MP3, tags the file as YouTube-sourced, and plugs into the end of
     `HeartAcquisitionCoordinator` through one small extension point (`IAcquisitionFallback`). Songs
     from hearts and import trickle only; not album hearts, not play.
  5. Gives up after 72 hours with a plain dashboard line (counts, no song list).
- **Moved to "after v1"** (they stay in `docs/research/ytdlp-fallback.md` section 12 as the later
  hardening list): the persisted given-up list with its own retention and caps, moved-aside corrupt
  store files, deletion of a user's records when the user is deleted, admin-view rules beyond "no
  user sees another's", the `RetryBetterDays` rule and the "YouTube copy" label, and the
  thirteen-setting surface (v1 keeps `Enabled`, `FailedAttempts`, `AttemptSpacingHours`,
  `MaxWaitHours`, `PerHour`, `PerUserPerHour`, `MaxMinutes`, each clamped on the server with a
  minimum of 1, and `FailedAttempts` at least 2). **Kept for safety** because they stop a wrong
  fetch or a leak: strict store entries (a missing or corrupt field never counts as "enough
  attempts", the store fails toward not fetching), a re-heart never resets the count, "enabled but
  not set up" counts as unavailable, tags that carry no user identity, a store file created `0600`,
  an A/B isolation test, and the free-space check before a download. Security confirms this cut in
  its review of the PR; anything it rejects moves back in. (This split is the Architect's reading;
  Security has not yet seen it.)
- **Module.** `octo/Services/YtDlpFallback/`, tests `octo.Tests/YtDlpFallback/`.
- **Switch.** `YTDLP_FALLBACK_ENABLED`, default **true once item 2 is merged** (D5). The PR may ship it
  `false` until pinning is in, and the same PR flips it when it is. The README states what it does and
  that yt-dlp is pinned.
- **Hotspots.** Common hotspots, plus `.github/workflows/ci.yml` (the shim scan job, its own job).
  Shared, not hotspots: `HeartAcquisitionCoordinator.cs`, `TrickleWorker.cs`, `SoulseekLink.cs`,
  `SoulseekClient.cs`, `SoulseekDownloadService.cs`, `LidarrHeartAcquisitionService.cs`,
  `YouTubeResolver.cs`, `LocalLibraryService.cs`, `yt-dlp-shim/app.py`, `yt-dlp-shim/Dockerfile`.
  List them in the PR.
- **Commits.** (1) shim hardening and pin, (2) typed outcomes with the FLA-31 fixes, (3) the
  extension point, (4) the module, switch and README. 1 and 2 are behaviour-preserving apart from the
  fixes and can merge first.
- **Harness.** `test_ytdlp_fallback.py`: (1) a song missing from stubbed Soulseek and Lidarr arrives
  through the stubbed shim and carries the YouTube-sourced tag; (2) during a stubbed Soulseek outage
  nothing is taken from YouTube; (3) alice's parked songs are not visible to bob. Waiting for 3 x 6
  hours is not testable in a run: the test pre-seeds a complete valid store entry with past times, and
  unit tests use a fake clock. Say so in the PR.
- **Security inherited.** Conditions 1 to 6, 8, 10, 11, 13 and 19 to 21 of
  `docs/research/ytdlp-fallback.md` section 12 (the safety set above); the rest are the "after v1"
  list. Security reviews the PR.
- **Depends on.** FLA-12 (done). FLA-31 is the same branch.

### FLA-31 Soulseek outage handling (inside FLA-21, phase 1)

Built on the FLA-21 branch as item 3 above. Its own acceptance criteria stand (outage waited out
whatever Lidarr's position, a Lidarr failure not read as "not found", tests for each shape, README
matches). If FLA-21 is delayed, the typed-outcome commit with the FLA-31 fixes can merge alone.

### FLA-22 Improve search results (phase 2)

- **Scope.** Library results stay on top and are never delayed or emptied by the outside half. The
  outside half gets Deezer track search as a second source next to Last.fm, merged by reciprocal
  rank with the existing `SongIdentity.MatchKey` dedupe. It works with no Last.fm key. One small
  Last.fm fix: `autocorrect=1` on the artist pad. Out of scope: type-ahead (`songCount` under 12 keeps
  skipping outside calls), library-side widening, YouTube Music as a source, a Last.fm measurement.
- **Robustness rules (D3).** The outside half has its own deadline. A Deezer or Last.fm error, timeout
  or empty answer returns the library rows plus whatever the other source gave; both failing returns
  the library only. Tests cover each case.
- **Module.** `octo/Services/SearchFusion/` with `ISearchSongSource`, a Last.fm source, a Deezer
  source (through the existing Deezer HTTP client so `DeezerRateLimiter` applies) and the fusion
  function. One hook: one branch in `ExternalSearchService.BuildAsync`; off runs today's path byte for
  byte. `SubSonicController`, `SearchBudget` and the paging classes are not touched.
- **Switch.** `SEARCH_FUSION_ENABLED`, default **true** (Deezer added, D3).
- **Hotspots.** Common hotspots; shared, not hotspots: `ExternalSearchService.cs`, one parameter in
  `LastFmService.cs`.
- **Acceptance numbers.** Fixture `docs/research/search-queries.json` with canned source answers: at
  least 12 of the 13 scored cases meet `minSongs` and `mustIncludeInTop` (the one allowed miss is
  `Mr. Brightside live`, which the fixture says needs two sources to agree), type-ahead makes zero
  outside calls, "wekend" is best effort, both scenarios pass. Latency no worse than today: measure
  the uncached outside build before and after on the same stubbed upstream and once live, keep the
  10 second deadline, and report both numbers on the PR (the human's reference is 2 to 4 seconds in
  the app today).
- **Harness.** `test_search_fusion.py` against the stub (the Deezer base URL must be configurable; if
  it is hard-coded, a setting is added in this issue). Seed: a library with exactly one match for the
  "daft punk" scenario. Plus a test that a stubbed outside failure still returns the library rows. No
  per-user data.
- **Security inherited.** R4.12 (fixed hosts), timeouts and size caps on both sources, R4.6 (titles
  are data, sanitised in logs). Security reviews only if a new host or URL setting is added.
- **Depends on.** FLA-12 (done).

### FLA-23 Build the Discover Weekly module (phase 2)

- **Scope (lean).** Our own small module. Candidates: ListenBrainz recommendations (for users whose
  name the module can prove from their own stored token) and library songs the user has never
  touched. No Explo, Troi or recommender library; no Last.fm similarity and no admin-typed
  ListenBrainz name map in v1 (both can come later). A fail-closed exclusion gate against the user's
  playlists and favorites (`docs/research/discover-weekly.md` section 4.2), so no song is in any of
  that user's playlists or favorites. One private "Discover Weekly" Navidrome playlist per user,
  created with the user's own credential, read back and checked private. **Every allowed user is
  covered unless they opt out** (D4). Up to 10 outside songs fetched per user per week through the
  existing acquisition entry points. No server-wide caps (D4); the per-user hourly rate and the
  existing acquisition checks (size, duration, free space) still apply.
- **The trigger.** Built during the user's first live request of the week: a filter the module
  registers itself starts a run only after `CredentialCheck` returns Accepted, for `u` with `t`/`s` or
  `p`, never an API key (API-key sign-ins get no list; the status says why). Nothing about the
  credential is stored. **Spike first** (design section 9). If it fails, one claimed line beside
  `EnsureAsync` in `GetPlaylists` in `SubSonicController.cs`; the sign-off covers that fallback only
  if the spike fails.
- **Opt-out.** A small panel on the dashboard for any signed-in user (the dashboard already serves
  non-admin Navidrome users for imports): says what the list does and lets the user turn it off.
  Opting out stops runs and deletes their rows.
- **Module.** `octo/Services/DiscoverWeekly/`, controller `octo/Controllers/DiscoverWeeklyController.cs`,
  panel files under `octo/wwwroot/admin` as a separate JS file (not edits inside `admin.js`).
  **Made for you is not edited** (the PR diff shows it).
- **Switch.** `DISCOVER_WEEKLY_ENABLED`, default **false** until the PR is merged and checked; the
  per-user cap `DISCOVER_WEEKLY_PER_USER_WEEKLY` default 10. Off: no calls, runs or files; endpoints
  say off.
- **Marking.** Songs it fetches are marked so the yt-dlp fallback declines them (section 4, a
  decision taken in this plan): an automatic pick should not turn into a YouTube MP3.
- **Hotspots.** Common hotspots; the `SubSonicController.cs` line only if the spike fails.
- **Harness.** `test_discover_weekly.py`: alice and bob (overlapping private playlists and favorites
  in the seed) each make a request and each get a private playlist; **no song in a user's list is in
  any of that user's playlists or favorites**; bob cannot read, change or delete alice's list (R2.7);
  "Alice" and "alice" share one cap; an API-key request starts nothing; a refused request creates no
  row; opt-out stops runs; switch off makes no run and writes no file. Library-derived candidates need
  no external stub.
- **Security inherited.** The binding conditions of `docs/research/discover-weekly.md` section 10 that
  stay relevant for the lean scope: 1 to 3, 5, 6, 8, 10 to 18 (credential never held; Accepted-only
  trigger; privacy read back; isolation test; external input rules; `0600` store; off by default; one
  case-folded user key; late-landing songs checked by the landed file's tags). Condition 7 (name map)
  and 9 (server-wide caps) no longer apply because the feature is out of scope or decided. Condition 4
  is moot as in the design. The PR returns to Security, trigger and credential code line by line.
- **Depends on.** FLA-12 (done). Starts after phase 1 (at most two builds at once).

### FLA-20 Admin setting: what each import source is used for (phase 3)

- **Scope.** One small dashboard page and a policy store. Options per source: (1) playlist only, (2)
  playlist and fetch missing songs (today's behaviour, the default), (3) playlist, fetch missing and
  favorites (needs FLA-19, built first). No "taste seed for Discover Weekly" option: Discover Weekly
  does not read imports, and a link would break "no import module depends on another".
- **Module.** `octo/Services/ImportPolicy/` with `IImportPolicy.For(source)` that the import code
  calls; the policy module never calls back. Store `import-policy.json` in `/app/config`. Endpoints in
  `octo/Controllers/ImportPolicyController.cs` under `api/admin/import-policy`.
- **Switch.** `IMPORT_POLICY_ENABLED`, default **true**; off means every source behaves as today and
  the page is hidden.
- **Hotspots.** Common hotspots; `octo/wwwroot/admin/index.html` and `admin.js` (one clearly marked
  block, or a separate file).
- **Harness.** `test_import_policy.py`: "playlist only" queues nothing, "fetch" queues, the setting
  survives an Octo restart, a non-admin is refused. Browser check at desktop and phone width.
- **Security inherited.** R3.1 to R3.3, R3.5, R3.6; values validated against the fixed option list.
- **Depends on.** FLA-19. FLA-18 is built after and calls the same interface.

### FLA-25 Verify updates on production installs (phase 3, a short verification only)

- **Scope.** The update path exists (`scripts/updater/`, `UpdateController`, `octo/Services/Updates`,
  tested by `scripts/updater/test-updater.sh` and `UpdateHostTests`). Do a short verification, not a
  build: one test that updates a disposable stack from the previous release to the current build and
  keeps `config`, `.env` and `docker-compose.override.yml`, and one that a failed update leaves the
  old version running. Fix a gap only if it is a bug; file anything bigger as an issue. Nothing
  pushes images or tags and nothing touches a real deployment.
- **Module.** Not a module. Tests in `tests/integration/` as their own workflow file.
- **Switch.** None.
- **Hotspots.** `.github/workflows/*` only to add its own file. README only for a real change.
- **Security inherited.** R3.8 (no new ports); the no-Docker-socket design stays.
- **Depends on.** FLA-12 (done).

### FLA-18 YouTube Music import, per user (phase 3, low priority)

- **Scope.** Low priority (D7); drop it if time is short. v1 needs **no stored login**: a public
  playlist link or id, and a Google Takeout file the user uploads. No cookies, OAuth tokens or request
  headers are stored. Reason: the shim's `ytm.py` has no authentication today, and a stored cookie is
  a full-account credential (R1.5). The private liked-music list is reached through Takeout only.
  Liked songs follow the FLA-19 rule (favorites on by default, playlists separate and private).
- **Module.** `octo/Services/YouTubeMusicImport/`, controller
  `octo/Controllers/YouTubeMusicImportController.cs` under `api/admin/imports/ytm/...`, dashboard files
  under `octo/wwwroot/admin`. Reuses `ImportStore`, `TrickleQueue`/`AcquisitionWorker` (no new queue),
  `ImportPlaylists` and the limits of `ImportFileReader` (R4.13).
- **Switch.** `YTMUSIC_IMPORT_ENABLED`, default **false**.
- **Hotspots.** Common hotspots; shared, not hotspots: `octo/Services/Imports/ImportModels.cs` (one
  new `ImportSources` constant) and, if the link path stays, a read-only playlist route in
  `yt-dlp-shim/ytm.py` and `app.py`. List both in the PR.
- **Harness.** `test_ytm_import.py`: alice and bob each upload a generated Takeout-shaped file; each
  sees only their own list; order kept; bob cannot read, refresh or delete alice's list by id. The
  link path, if kept, uses a stub route.
- **Security inherited.** R1.5 (nothing stored), R2.1 to R2.3 and R2.7, R4.12 (the link is parsed to
  an id; Octo builds the host), R4.13, R4.1 and R4.6. Security approves the PR.
- **Depends on.** FLA-19 (same code), FLA-21's shim hardening (titles go through its choke point).

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
2. **Phase 2:** FLA-22 and FLA-23, each started when a phase 1 slot is free.
3. **Phase 3:** FLA-20 and FLA-25, then FLA-18.
4. **Phase 4:** FLA-26, then FLA-27.
5. FLA-11 any time after FLA-28 merges (nothing in this plan touches cover code).

| Overlap | Handling |
|---|---|
| Acquisition chain (`HeartAcquisitionCoordinator`, `TrickleWorker`, `SoulseekLink`) | FLA-21 and FLA-31 are one branch. |
| `octo/Services/Imports` | FLA-19, then FLA-20, then FLA-18. |
| The verified-request registration point | FLA-19 (pending stars) and FLA-23 (trigger) both need it. FLA-19 is first, so it proves the point (or uses the fallback line); FLA-23 copies the pattern, it does not share internals. |
| Shim and `YouTubeResolver` | FLA-21 first; FLA-18 later goes through the choke point it builds. |
| Shared stub | FLA-21 creates it; later modules add routes in their own files. |
| `admin.js` | FLA-20 and the Discover Weekly panel each use a separate JS file. |
| The five common hotspot files | Merge in order; each next branch rebases on `main` before review; never drop someone's lines. |

Review rule per PR: QA verifies against its criteria; Security reviews FLA-18, FLA-19, FLA-21, FLA-23
and any change that adds a host, port or admin route; the orchestrator merges verified PRs (D12).

---

## 4. Open questions for the human

Only two remain. Everything else from the first draft is answered by the decisions in section 1.

1. **Deezer's terms.** *Recommend: one person reads Deezer's published terms for the public API
   before FLA-22 is merged.* Deezer is already used by default for albums and enrichment without a
   key, and FLA-22 adds one more request per search of 12 songs or more, so I expect no change, but I
   did not read the terms. The build can proceed meanwhile; the switch is one setting if the answer is
   no.
2. **"A requested song plays right away and is downloaded in the background" (D6).** Octo already can
   stream an outside song from YouTube at the first byte and download it on play, but
   `DOWNLOAD_ON_PLAY` is off by default (`docs/architecture.md` section 3.2). *Recommend: confirm that
   you want it on by default.* If yes, it is a one-line default change with its README line, filed as
   a small separate Backend issue, not part of this plan's builds.

A decision I took inside the plan that you may overrule: the yt-dlp fallback declines songs that
Discover Weekly fetches (FLA-23 section), so an automatic pick never becomes a YouTube MP3.

---

## 5. Risks, and what to cut

| Risk | Mitigation |
|---|---|
| The Discover Weekly trigger does not work as designed | Do the spike first; the fallback is one claimed line in `SubSonicController.cs`; if both fail, ship it as a dashboard "Build now" button (a dashboard sign-in is also a verified request) |
| FLA-21 touches the most shared code | Hardening and typed outcomes first, behaviour-preserving, each tested; off means off is tested; Security reviews the shim change before the module |
| Default-on for the yt-dlp fallback | The pin and the shim scan ship in the same PR; Security re-reviews when the default is on |
| The harness cannot test time | Fake clock in unit tests; the harness pre-seeds valid entries; each PR says so |
| Unverified upstream behaviour (Deezer terms, ListenBrainz `validate-token`, Navidrome API details) | Each is a build-time spike with a stated stop condition |
| Accepted High advisories on ImageSharp 3.1.12 (D8) | The size cap is in; FLA-32 evaluates a replacement; FLA-27 reports it |
| CI evidence | Docs-only PRs used `[skip ci]` and had no CI run to cite; a build PR must confirm `ci.yml` and `integration.yml` actually ran, and say so when they did not |
| Auto-downloading at volume | Per-user cap of 10 a week for Discover Weekly and the yt-dlp per-hour limits; each is a setting |

**What I would cut if time is short, in order:** FLA-18, then FLA-20, then FLA-25's tests beyond the
failed-update case. I would not cut FLA-19, FLA-21 with FLA-31, FLA-22, FLA-26 or FLA-27. If FLA-23's
trigger spike fails twice, ship it as the "Build now" button instead of dropping it.
