# Plan for Wave 2 and the final checks

The detailed plan that FLA-17 asks for. It pulls the architecture map, the module conventions,
the security requirements, the four research and design documents and the human decisions of
2026-10-10 into one place, then says what to build, in what order, and what is still open.

**Status: draft for human sign-off.** Nothing in Wave 2 starts until a human has approved the
stack and priorities on FLA-17 (step 2 of that issue). The Wave 2 issues are not yet updated
with this scope (step 3); that follows the approval.

Sources, all on `main`: `docs/architecture.md` (FLA-7), `docs/modules.md` (FLA-8),
`docs/security-requirements.md` (FLA-9), `docs/testing.md` (FLA-12), `docs/research/search.md`
(FLA-13), `docs/research/ytdlp-fallback.md` (FLA-14), `docs/research/discover-weekly.md`
(FLA-15), `docs/research/retention.md` (FLA-16). Line numbers are not repeated here; each
source document cites the code it was read from.

---

## Summary (one screen)

**What gets built, in this order**

| Step | Issues | What the user gets | Size |
|---|---|---|---|
| A (start together) | FLA-19 | Imports keep order, star liked songs for that user only, stay private; proven by tests | small to medium |
| A | FLA-21 with FLA-31 (one branch) | yt-dlp as last resort after 3 failed attempts, saved as MP3; outages still waited out; switch off | large |
| A | FLA-22 | Search finds more outside songs, also without a Last.fm key (Deezer track search fused with Last.fm) | medium |
| A | FLA-23 | Discover Weekly: one private playlist per user, built on their first request of the week, no credential held, 10 auto-downloads per user per week; switch off | large |
| A (any time, disjoint files) | FLA-25 | Proof that the updater works end to end | small to medium |
| B | FLA-18 | YouTube Music import per user (recommended v1: public playlist link and Google Takeout file, no stored login) | medium |
| B | FLA-20 | Admin page: what each import source is used for | small |
| C | FLA-26, FLA-27 | Regression with modules on and off, dependency scans in CI, final security review | medium |
| Postponed | FLA-24 | 30-day retention cleanup. Not built. The design stays as the record | none |

**Decided already (details in section 1):** retention postponed; Discover Weekly with no held
credential and 10 downloads per user per week, switch off; yt-dlp after several failed
Soulseek or Lidarr attempts, MP3, outages waited out, switch off; ImageSharp 4.x approved; GitHub
Actions runs CI only; the orchestrator may merge verified PRs.

**Every new module** is one folder under `octo/Services/<Module>`, one switch with its default in
four places, one registration line in `Program.cs`, its own tests, and a validation workflow in the
integration harness. Off means off. Every module that touches auth, user data, credentials,
external input or files comes back to Security as a PR review.

**What I need from you (section 4 has the reasoning, with my recommendation for each):**

1. Deezer as a second search source: approve, with the default on.
2. Last.fm: do not wait for a real key; build against canned answers and record a live run when a key exists.
3. Keep type-ahead (`songCount` under 12) free of outside calls.
4. Discover Weekly: opt-out coverage, but only with a user-reachable opt-out in the dashboard; otherwise opt-in.
5. Discover Weekly server-wide caps: 100 songs a week, 20 an hour, 10 GB free floor.
6. API-key sign-ins get no Discover Weekly list in v1.
7. yt-dlp switch stays off; file one follow-up issue to pin and verify yt-dlp, which unblocks default-on later.
8. YouTube Music import v1: public link and Takeout file only; no cookies or login stored.
9. Import fidelity: how liked songs become stars without holding a password.
10. FLA-22 thresholds and latency budget (proposed in section 2).
11. Should the yt-dlp fallback decline songs that Discover Weekly fetches?
12. A shared stub service for the harness (a test-only container).
13. The remaining Medium findings from the security audit (SEC-3 to SEC-6, SEC-9): file them or not.
14. Split the dependency-scan part out of FLA-26 so it lands early.

**What I would cut if time is short, in order:** FLA-20, then the Takeout/link breadth of FLA-18,
then FLA-25 (the updater already has shell tests). I would not cut FLA-19, FLA-21, FLA-22, FLA-26
or FLA-27. FLA-23 is the riskiest build; if its trigger spike fails, the fallback in section 3
applies before anything is cut.

---

## 1. Decisions already made by the human

All quoted or recorded on the issue named, on 2026-10-10, relayed by the orchestrator. Under the
rule the human set that day, a quoted relay is written approval for the action and scope it names.

| # | Decision | Recorded on |
|---|---|---|
| D1 | The retention build **FLA-24 is postponed**: "Lohnt sich das Feature überhaupt? Dann lassen wir das mal weg." It stays in Backlog and nobody starts it until a human lifts the postponement. The design (`docs/research/retention.md`, with Security's 19 conditions) is kept as the record. | FLA-16, FLA-24 |
| D2 | Retention design choices kept anyway: 1 to 2 star ratings protect a song, an album or artist heart protects all its songs, Navidrome is not pinned. Only matters if D1 is ever lifted. | FLA-16 |
| D3 | **Discover Weekly holds no credential.** The list is built during the user's first live request of the week, using the freshly verified credential of that request, used for that run only, never stored. A user who is not active that week gets no new list. | FLA-15 |
| D4 | **Discover Weekly auto-downloads are on for each user, capped at 10 songs per user per week**, and the module switch ships **off**. A starred artist does not exclude all of that artist's songs. | FLA-15 |
| D5 | **The yt-dlp fallback runs after several failed Soulseek or Lidarr attempts** (3 counted attempts at least 6 hours apart by default), not after one miss. The file is saved as **MP3**. A Soulseek or Lidarr **outage** is waited out and its attempts do not count. A Lidarr failure never blocks the fallback forever. Gives up after 72 hours and shows it on the dashboard. The switch stays **off** for now. | FLA-14 |
| D6 | The other recommendations of the FLA-14 design are accepted: the small coordinator hook, album hearts later (separate switch, separate issue), the legacy order unchanged. | FLA-14 |
| D7 | **ImageSharp 4.x is approved** (4.1.x with the matching ImageSharp.Drawing major version, plus the new transitive packages PolygonClipper and System.IO.Hashing). Scope: those packages and the code changes in `octo/Services/CoverArt/*`, nothing else. | FLA-28 |
| D8 | **GitHub Actions runs CI only.** `docker.yml` and `release-links.yml` are disabled, so a merge to `main` does not publish images. | memory file, orchestrator |
| D9 | **The orchestrator may merge verified PRs.** Decisions given in chat and relayed verbatim to Linear count as approval. Tags, releases, deployments and running the updater against a real server still need a human each time (`AGENTS.md`). | memory file, orchestrator |
| D10 | All agent work runs on Sonnet (the human's instruction), overriding the model table in `AGENTS.md`. One commit per finished issue. | memory file |

Open and **not** decided, carried into section 4: whether the yt-dlp switch ever defaults on
(the human wants more context on Security condition 11).

Already built or in flight and not part of this plan: FLA-12 (harness, merged), FLA-29 (slskd
default login, merged), FLA-28 (ImageSharp, in progress), FLA-30 (quarantine real path, in
progress), FLA-10 (layout jump, skipped by the human, left in Todo), FLA-11 (new cover PNG, waits
for FLA-28 because both edit `CoverArtService.cs`).

---

## 2. One section per Wave 2 build issue

Conventions used below. "Switch" is the on/off setting; every switch states one default in
`.env.example`, `docker-compose.yml`, `octo/appsettings.json` and the settings class
(`docs/modules.md`, "The switch"). "Hotspots" are the shared files that need a claim
(`AGENTS.md`); each module changes only its own line or paragraph in them. "Harness" is the
integration harness in `tests/integration/` (`docs/testing.md`): each module adds seed data, at
least one `test_<module>.py`, a per-user isolation check for anything stored per user, and any
stub it needs in `tests/integration/docker-compose.yml`.

Hotspot set common to every new module: `octo/Program.cs` (one registration line),
`.env.example`, `docker-compose.yml`, `octo/appsettings.json`, `README.md` (one paragraph). It is
written "common hotspots" below, with extras named.

### FLA-18 YouTube Music import, per user

- **Recommendation.** Ship v1 with sources that need **no stored login**: (a) a public playlist
  link or id, (b) a Google Takeout file the user uploads (playlists and liked music). Do not store
  YouTube cookies, OAuth tokens or request headers in v1. Why: the shim's `ytm.py` has no
  authentication code at all today (search, radio and artist radio only), `ImportSources` has only
  `spotifyLiked`, `spotifyPlaylist`, `link` and `file`, and a stored cookie is a full-account
  credential, the highest class under R1.5. The Takeout path is also the only one the harness can
  test without a stub of YouTube.
- **Module.** `octo/Services/YouTubeMusicImport/` (`Octo.Services.YouTubeMusicImport`), its own
  controller `octo/Controllers/YouTubeMusicImportController.cs` (routes under `api/admin/imports/ytm/...`
  so the admin gate and the signed-in-user pattern of `ImportsController` apply), dashboard
  additions under `octo/wwwroot/admin`.
- **Switch.** `YTMUSIC_IMPORT_ENABLED`, default **false** for v1. It adds an
  outbound path to YouTube Music for playlist reads. Settings section `YouTubeMusicImport`.
  Off removes the dashboard section and the endpoints answer "off".
- **Reuse.** Stores lists in `ImportStore` as `ImportList` with `Owner`, fetches missing songs through
  the existing `TrickleQueue`/`AcquisitionWorker` (acceptance: no new queue), writes the playlist
  with `ImportPlaylists`. File parsing reuses the limits of `ImportFileReader` (64 MB, 200 entries,
  256 MB unpacked, R4.13).
- **Hotspots.** Common hotspots, plus two shared files that are not hotspots but belong to another
  feature: `octo/Services/Imports/ImportModels.cs` (one new constant in `ImportSources` and its
  `CanRefresh` rule) and `yt-dlp-shim/ytm.py` / `yt-dlp-shim/app.py` (one read-only
  playlist route, if the public-link path is kept). List both in the PR.
- **Harness.** `tests/integration/test_ytm_import.py`: alice and bob each upload a Takeout-shaped
  fixture (generated, no real data); each sees only their own list; order is kept; bob cannot read,
  refresh or delete alice's list by id. If the public-link path stays, a stub shim route serves a
  canned playlist; this needs the shared stub (question 12).
- **Security conditions inherited.** R1.4, R1.5 (nothing stored in v1, so these hold trivially;
  any later cookie support returns to Security), R2.1 to R2.3 and R2.7 (per-user lists), R4.12
  (the playlist link is parsed into an id and the host is built by Octo, as `SpotifyLinkReader`
  does), R4.13 (upload limits), R4.1 and R4.6 (titles are data, never a shim argument starting
  with `-`). Security approves the PR, as the issue says.
- **Depends on.** FLA-17 (gate), FLA-12 (done). **Practically after FLA-19**, because both edit
  `octo/Services/Imports`, and "liked songs handled like Spotify" needs FLA-19's favorites rule.
- **Open questions.** Questions 8 and 9 in section 4.

### FLA-19 Import fidelity: order, favorites, private playlists

- **Recommendation.** Treat this as an audit-and-fix of three behaviours already half there.
  Per `docs/architecture.md`: order (A6) and privacy (A8) **exist** (`ImportFileReader.ReadCsv`
  keeps row order, `ImportPlaylists.SyncAsync` writes in list order and keeps the matching prefix,
  playlists are created `public = false` and handed to the owner); favorites (A7) are **missing**
  (the liked list becomes a playlist named "Liked Songs" and nothing stars a song).
  Step 1: write the three harness tests first. Order and privacy tests will pass on current code and
  act as locks; the "tests fail on the current code" criterion can only be met for favorites and
  for any real defect the order test finds after songs arrive later. Say so in the PR. Step 2:
  fix what fails.
- **Favorites mechanism (decision needed, question 9).** Octo never holds a password and cannot act
  as a user without a live credential (`docs/architecture.md`, finding 2; R2.5 forbids an admin
  shortcut around Navidrome's ownership checks). Recommended: a **per-user "pending stars" list
  in the import store (song ids only)** that is applied on that user's next live Subsonic request,
  with that request's verified credential, using the same pattern FLA-23 uses for its trigger
  (verified by `CredentialCheck`, never stored, canonical user key). Stars apply to songs already
  in the library at once and to fetched songs as they land. Not recommended: holding a token for a
  day (the design FLA-15 rejected for the same reason).
- **Module.** Not a new module: fixes in `octo/Services/Imports` (existing code, convention does not
  apply retroactively, `docs/modules.md`). New code for the pending-stars hook goes in its own file
  `octo/Services/Imports/ImportStarsOnRequest.cs`, behind the import policy of FLA-20 once that
  exists. Switch: none new for order and privacy (bug fixes); the favorites behaviour gets
  `IMPORT_STAR_LIKED`, default **false** until the human has seen it work, since it writes to users'
  Navidrome data.
- **Hotspots.** Common hotspots for the one switch. Shared, not hotspots: `ImportService.cs`,
  `ImportPlaylists.cs`, `ImportStore.cs`. If the request hook cannot be registered by the module
  itself, a one-line call in `SubSonicController*.cs` needs a claim (same spike as FLA-23).
- **Harness.** `test_import_fidelity.py`: (1) CSV import of a 6-row list, some songs present, others
  arriving later; playlist order equals source order before and after arrival; (2) liked songs are
  starred for the importing user and for nobody else (alice and bob overlap on songs on purpose, so
  a leak shows); (3) the imported playlist is non-public and owned by the importer, checked through
  Navidrome directly and through Octo as bob.
- **Security conditions inherited.** R2.1 to R2.5 and R2.7 (user from a verified sign-in, Navidrome's
  own checks, A/B test), R1.2 (the live credential is never logged or stored, as FLA-15 conditions 1
  and 14). Touches user data and credentials, so **Security reviews** the PR.
- **Depends on.** FLA-17, FLA-12. No dependency on other Wave 2 issues. **Build first in step A.**
- **Open questions.** Question 9.

### FLA-20 Admin setting: what each import source is used for

- **Recommendation.** One small dashboard page and one policy store, so each import source has a
  setting. Option list (my proposal for the "final option list" the issue asks the gate for):
  1. **Playlist only** (keep as a Navidrome playlist, fetch nothing, star nothing)
  2. **Playlist and fetch missing songs** (today's Spotify behaviour)
  3. **Playlist, fetch missing and star liked songs** (needs FLA-19)
  Drop the "taste seed for Discover Weekly" option the architecture map mentioned: the Discover Weekly
  design takes taste from the user's Navidrome plays, their ListenBrainz account and library songs,
  not from imports, and a cross-module dependency would break "no import module depends on another".
  The default for an existing source is whatever it does today (option 2), so enabling nothing
  changes nothing.
- **Module.** `octo/Services/ImportPolicy/` with a tiny public interface (`IImportPolicy.For(source)`
  returning the option) that import modules call; the policy store `import-policy.json` in
  `/app/config`. The Imports code and FLA-18 ask the interface; the policy module never calls into
  them. Admin endpoints in `octo/Controllers/ImportPolicyController.cs` under `api/admin/import-policy`
  (admin session, `AdminRequestGuard`, no GET-side changes).
- **Switch.** `IMPORT_POLICY_ENABLED`, default **true** (it is only a policy lookup that defaults
  to today's behaviour). Off: every source behaves as today and the page is hidden.
- **Hotspots.** Common hotspots; `octo/wwwroot/admin/index.html` and `admin.js` (the shared
  dashboard files; additions in a clearly marked block only). `admin.js` is large and also touched
  by FLA-10 if that is ever resumed.
- **Harness.** `test_import_policy.py`: set a source to "playlist only", import as alice, no songs
  are queued; set to fetch, songs are queued; restart Octo, the setting survives; a non-admin gets a
  refusal. Browser check at desktop and phone width for the page (criterion), screenshots on the PR.
- **Security conditions inherited.** R3.1 to R3.3, R3.5, R3.6 (admin routes, guard, no-store); setting
  values validated against the fixed option list on the server.
- **Depends on.** FLA-17, FLA-12; **practically after FLA-19 and FLA-18**, because it must offer the
  options those two define.
- **Open questions.** None beyond the option list above, which is part of the sign-off.

### FLA-21 Build the yt-dlp fallback (with FLA-31)

- **Recommendation.** Build the design in `docs/research/ytdlp-fallback.md` as written: a module
  that plugs into the end of the acquisition chain through one small extension point, counts
  failed attempts, waits out outages, and downloads through the existing YouTube path as MP3. Fix
  **FLA-31 in the same branch**: the design's typed source outcomes (`NotFound` versus `Unavailable`)
  are exactly what FLA-31 needs (Soulseek outage held whatever Lidarr's position, a Lidarr error
  not read as "not found"). Doing them separately would edit the same lines in
  `HeartAcquisitionCoordinator.cs`, `TrickleWorker.cs`, `SoulseekLink.cs` twice.
- **Module.** `octo/Services/YtDlpFallback/` (design section 5.1), tests in `octo.Tests/YtDlpFallback/`.
- **Switch.** `YTDLP_FALLBACK_ENABLED`, default **false** (D5). Thirteen further settings with
  bounds, listed in design section 5.1 (Security conditions 15 and 16 require server-side
  clamping and tests for 0, negative, huge and non-numeric input). Known build detail from the
  FLA-14 thread: `MaxWaitHours` has a computed minimum that can exceed its stated maximum (726
  against 720 at `FailedAttempts` 10 and spacing 72); the computed minimum must win, with a test.
- **Commits (design section 10).** 1 shim hardening (video id, `%` escape, `dest` confined to
  `.octo-incoming`, `--`, limits; closes SEC-8), 2 typed outcomes (also FLA-31), 3 the extension
  point in the coordinator, 4 the module, 5 `RetryBetterDays` and the "YouTube copy" label.
  Commits 1 and 2 are useful alone and should be merged first.
- **Hotspots.** Common hotspots; shared non-hotspots the PR must list: `HeartAcquisitionCoordinator.cs`,
  `SoulseekClient.cs`, `SoulseekDownloadService.cs`, `LidarrHeartAcquisitionService.cs`,
  `YouTubeResolver.cs`, `LocalLibraryService.cs`, `QualityUpgradeWorker.cs`, `TrickleWorker.cs`,
  `SoulseekLink.cs`, `yt-dlp-shim/app.py`. This is the widest footprint in Wave 2: see the ordering
  in section 3.
- **Harness.** `test_ytdlp_fallback.py` (both required criteria): (1) a song missing from stubbed
  Soulseek and Lidarr arrives through a stubbed shim and carries the YouTube-sourced tag and mapping
  flag; (2) during a stubbed Soulseek outage nothing is taken from YouTube. Real waiting (3 attempts,
  6 hours apart) is not testable in a run; the test pre-seeds `ytdlp-fallback.json` with valid
  counted attempts in the past (Security condition 20 allows only fully valid entries, so the seed
  must be complete), and the unit tests drive a fake clock. Needs stubs for slskd, Lidarr and the
  shim in the harness compose (question 12). Isolation check: alice's parked and given-up songs
  are invisible to bob, and the admin read returns counts only (condition 18).
- **Security conditions inherited.** All 21 in `docs/research/ytdlp-fallback.md` section 12 (shim
  input validation, `dest` confinement, query cleaning at one choke point, log hygiene, size and
  duration limits enforced in the shim, per-user and global rate limits, `0600` store, tags that
  carry no user identity, SEC-7 as a precondition for **default-on**, bounded settings, deletion with
  the user, counts-only admin view, "enabled but not set up" counts as unavailable, strict store
  entries, no reset by re-hearting). Security reviews the PR (external input, shim, files, per-user
  data).
- **Depends on.** FLA-17, FLA-12. Related: FLA-31 (same branch), FLA-14 (design).
- **Open questions.** Question 7 (default-on and the yt-dlp pin), question 11 (Discover Weekly
  songs).

### FLA-22 Improve search results

- **Recommendation.** Option B of `docs/research/search.md` with the cheap parts of A: add Deezer
  track search as a second outside source, run in parallel with Last.fm, fuse by reciprocal rank
  with the existing `SongIdentity.MatchKey` dedupe (and `LooseKey` as a second chance), and fix the
  Last.fm anchor (`autocorrect=1`, artist found through `artist.search` rather than the raw query).
  It is the only option that fixes the key-less case. Out of scope: type-ahead, library-side
  widening, YouTube Music as a third source (a later, separately switched issue if gaps remain).
- **Module.** `octo/Services/SearchFusion/` with `ISearchSongSource`, `LastFmSearchSource`,
  `DeezerTrackSearchSource` (through the existing Deezer HTTP client so `DeezerRateLimiter`
  applies) and the fusion function. One hook: `ExternalSearchService.BuildAsync` gains a single
  branch (fusion on: collect from `SearchFusion`; off: today's path unchanged). `SubSonicController`,
  `SearchBudget` and the paging classes are not touched.
- **Switch.** `SEARCH_FUSION_ENABLED`, default **true** (recommended, question 1). Off means
  byte-for-byte today's results and no Deezer track call.
- **Hotspots.** Common hotspots; shared non-hotspots: `octo/Services/Common/ExternalSearchService.cs`
  and one parameter in `LastFmService.cs`.
- **Acceptance numbers the issue leaves to this gate (my proposal, question 10).**
  *Result threshold:* on the 15-query fixture `docs/research/search-queries.json` with canned
  source answers, at least **12 of the 13 scored cases** meet their `minSongs` and
  `mustIncludeInTop`; the typeahead case makes **zero** outside calls; the "wekend" case is best
  effort and not scored; both scenarios pass (songs without a Last.fm key; page one shows the library
  row first then at least 15 outside rows). The one allowed miss is `Mr. Brightside live`, which the
  fixture itself says needs a second source to agree. *Latency budget:* an uncached outside build
  with fusion on takes at most the **median of today's plus 300 ms** and p95 plus 500 ms, measured
  before and after on the same stubbed upstream and once live; the 10-second build deadline is
  unchanged; the library rows are never delayed by the outside half.
- **Harness.** `test_search_fusion.py` against a stubbed Deezer and Last.fm in the harness compose
  (needs the Deezer base URL to be configurable; if it is hard-coded, a setting is added as part of
  this issue). Seed: a library holding exactly one matching song for the "daft punk" scenario.
  No per-user data, so no isolation test; search results are not per person.
- **Security conditions inherited.** No design-specific list was needed; the general rules apply:
  R4.12 (fixed hosts, no user-supplied URL), timeouts and response size caps on both sources,
  R4.6 (titles and artists are data), and the same sanitising before any log line. Security reviews
  only if the PR adds a new outbound host or setting that takes a URL.
- **Depends on.** FLA-17, FLA-12. Human confirmation of Deezer volume before the build starts (QA
  asked for this on FLA-13).
- **Open questions.** Questions 1, 2, 3, 10.

### FLA-23 Build the Discover Weekly module

- **Recommendation.** Build `docs/research/discover-weekly.md` as revised for D3 and D4: candidates
  from ListenBrainz (Weekly Exploration, recommendations, seeded similarity), Last.fm similarity
  and library songs the user has never touched; a three-layer fail-closed exclusion gate against the
  user's playlists, favorites and starred albums; one private "Discover Weekly" Navidrome playlist per
  user, created with the user's own credential; at most 10 outside songs fetched per user per week.
  No Explo, Troi or recommender library, and no new container.
- **Module.** `octo/Services/DiscoverWeekly/` (settings, recommendations client, pure selector with
  the exclusion gate, store and ledger, credential-free worker, module extension), its own controller
  `octo/Controllers/DiscoverWeeklyController.cs` (status and per-user opt-out), dashboard files for
  the opt-out and status. **Made for you is not edited** (the PR diff shows it).
- **Switch.** `DISCOVER_WEEKLY_ENABLED`, default **false** (D4). Per-user cap
  `DISCOVER_WEEKLY_PER_USER_WEEKLY` default 10. Server-wide caps in question 5. Off: no calls, no
  runs, no files, endpoints say "off".
- **The trigger.** A filter the module registers itself, which starts a run only after
  `CredentialCheck` returns Accepted for a `u` with `t` and `s` or `u` with `p`; no `apiKey`/`jwt`;
  one case-folded user key; query string only; drops when the check cap is full. **Spike first**
  (design section 9): prove an `IStartupFilter` registered from the module runs after
  `UseAdminPortSplit` and leaves the `Program.cs:725` body buffering alone. **Fallback if the spike
  fails:** one line beside `EnsureAsync` in `GetPlaylists` in `SubSonicController.cs`, a hotspot that
  needs a claim and, per design section 8, a human yes (question list in the design; I fold it into
  the sign-off of this plan: approved here means approved if and only if the spike fails).
- **Hotspots.** Common hotspots; the `SubSonicController.cs` fallback line only if the spike fails.
- **Harness.** `test_discover_weekly.py`: alice and bob have overlapping private playlists and
  favorites (the seed already does); after each makes a Subsonic request, each has a private
  "Discover Weekly" playlist; **no song in a user's list is in any of that user's playlists or
  favorites** (the required criterion); bob cannot read, change or delete alice's playlist id, ledger
  or picks (R2.7); "Alice" and "alice" share one cap; an API-key request starts nothing; a refused
  request with a real user name creates no row and does not suppress that user's run (condition 13);
  switch off makes no run and writes no file. Library-derived candidates need no external stub, so the
  core test needs only the existing seed; a ListenBrainz stub is optional.
- **Security conditions inherited.** All 19 in `docs/research/discover-weekly.md` section 10
  (credential only in local variables and never in a field, log or exception; coverage and the
  opt-out requirements; trigger rules 3, 13 to 17; playlist privacy read back and verified;
  isolation test; ListenBrainz name mapping proved or admin-only; external input rules; abuse caps;
  `0600` stores; off-by-default; PR returns to Security, trigger and credential code reviewed line by
  line; late-landing songs checked by the landed file's tags).
- **Depends on.** FLA-17, FLA-12. No dependency on FLA-21.
- **Open questions.** Questions 4, 5, 6, 11.

### FLA-24 Build the 30-day retention cleanup (POSTPONED)

- **Status.** Postponed by the human on 2026-10-10 (D1). Stays in Backlog; **no agent starts it**,
  even when its blockers are Done, until a human lifts the postponement in writing.
- **If it is ever lifted.** The design and Security's 19 conditions are in
  `docs/research/retention.md` (sections 9 to 11); the human has already decided ratings protect a
  song, an album or artist heart protects all its songs, and Navidrome is not pinned (D2). Still
  needed then: the credential option (A recommended), the snapshot trigger place, and written approval
  to run with `RETENTION_DRY_RUN=false`. FLA-30 (real-path check in quarantine) is already in
  progress and is a precondition Security named.
- **Effect on this plan.** FLA-26 and FLA-27 do not wait for it and do not review it.

### FLA-25 Verify updates on production installs

- **Recommendation.** Do it as the issue says: a harness-style test that updates a stack from the
  previous release to the current build, keeps `config`, `.env` and `docker-compose.override.yml`,
  and leaves the old version running after a failed update. It needs no gate and can start now.
  Note one constraint from `AGENTS.md` and decision D8: nothing may push images or tags, and the
  updater (`scripts/updater/`) is never run against a real server. The test uses a disposable
  stack and a local registry or locally built images only.
- **Module.** Not a module: tests and fixes in `scripts/updater/`, `scripts/test-install.sh`,
  `octo/Services/Updates`, `UpdateController`. No switch. Test goes in `tests/integration/` (or a
  sibling folder if it cannot share the Navidrome stack) as its own workflow file.
- **Hotspots.** `.github/workflows/*` only if a new workflow file is added (its own job, no edit to
  existing ones). README only for a documented change.
- **Security conditions inherited.** R3.8 (no new published ports), the updater's no-Docker-socket
  design stays; Security reviews if the updater scripts change.
- **Depends on.** FLA-12 (done). Not FLA-17.
- **Open questions.** None. Which "previous release" to start from is chosen by the build (the last
  tagged release is `2026.10.05` on `main`; the `2026.10.07` commit was reverted).

### FLA-26 Full regression with modules on and off, and security scans in CI

- **Recommendation.** After all Wave 2 builds. Matrix: harness with every new module on, and with
  every new module off (Wave 2 modules all default to off or to today's behaviour, so "off" is also
  the default install). Dependency scans: `dotnet list package --vulnerable --include-transitive`,
  `cargo audit`, `pip-audit`, plus a container image scan; High or Critical fails the build
  (`docs/security-requirements.md` R6.3 to R6.4, SEC-7). Existing CI jobs unchanged apart from
  additions. No test is skipped; flaky ones are fixed or filed (the one flaky test seen so far is
  `SearchAlbumOwnershipTests` under load, per the FLA-28 thread).
- **Split proposal (question 14).** The scan jobs do not depend on any module. Land them early as
  their own PR so a known-vulnerable dependency stops new work sooner, and so SEC-7 is on its way
  before anyone asks for the yt-dlp default to flip.
- **Hotspots.** `.github/workflows/*` (its own new jobs or files; no behaviour change to `ci.yml`,
  `docker.yml`, `release-links.yml`). Note D8: `docker.yml` stays disabled on this fork.
- **Depends on.** All Wave 0 UI fixes and all Wave 2 builds (except FLA-24, postponed).

### FLA-27 Final security review of all new modules

- **Recommendation.** One pass against every requirement in `docs/security-requirements.md`, marked
  pass or fail, with the status of every FLA-9 finding (SEC-1 and SEC-2 are covered by FLA-28 and
  FLA-29; SEC-3 to SEC-13 need a status each, see question 13). Focus areas as in the issue:
  cross-user access (imports, Discover Weekly, favorites, playlists), inputs into the shim and
  Soulseek, credential storage for import sources, and the conditions listed per module above. Retention
  is out of focus while FLA-24 is postponed; the report says so.
- **Output.** A Linear document on the project; no release approval (the issue is explicit).
- **Depends on.** All Wave 2 builds.

---

## 3. Recommended build order and parallelism

### Where the work overlaps

| Overlap | Who | What to do |
|---|---|---|
| The acquisition chain: `HeartAcquisitionCoordinator`, `TrickleWorker`, `SoulseekLink`, `SoulseekDownloadService` | FLA-21 and FLA-31 | One branch, one agent. FLA-31 is the typed-outcome work FLA-21 needs anyway. Two branches would conflict in the same lines. |
| `CoverArtService.cs` | FLA-28 (in progress) and FLA-11 | FLA-11 waits for FLA-28's merge. Already recorded. Nothing in Wave 2 touches cover code. |
| `octo/Services/Imports` | FLA-19, FLA-18, FLA-20 | Serialise: FLA-19, then FLA-18, then FLA-20. FLA-19 and FLA-18 both edit `ImportModels.cs`, `ImportService.cs` and the Imports controller. |
| The per-request, verified-credential trigger | FLA-23, FLA-19 (pending stars) | Same pattern, two modules. Do **not** share internals between modules. Build the FLA-23 spike first; FLA-19 reuses what the spike proves (the filter registration point) as a copied pattern, or, if the human agrees, a separate small issue extracts a shared "verified request hook". |
| `ExternalSearchService.cs`, `LastFmService.cs` | FLA-22 only | No overlap. |
| Shim `yt-dlp-shim/app.py` | FLA-21 (hardening, limits), FLA-18 (playlist read route, if kept) | FLA-21's shim commit lands first; FLA-18 adds its route after, and the shim hardening (video id, input limits) then covers it. |
| `YouTubeResolver.cs`, YouTube path | FLA-21 (input choke point), FLA-18 (titles into the shim) | FLA-18 goes through the choke point FLA-21 builds. Another reason for FLA-18 after FLA-21's first two commits. |
| `tests/integration/docker-compose.yml` | every module adds stubs | A merge-conflict factory. Proposed: each module adds its stubs in its own compose file, and the harness picks them up (a small QA change to `run.sh`, question 12). |
| The five common hotspot files | all modules | Each change is a few additive lines, so conflicts are easy but constant. Rule: merge in order, each next branch rebases on `main` before review (`AGENTS.md`), never resolved by dropping someone's lines. |

### Waves

**Step 0 (before Step A, a few hours):** QA makes the harness accept per-module stub compose files
(and optionally the shared stub service, question 12). Backend files the yt-dlp pin issue
(question 7). Both are small and unblock the rest.

**Step A (start together, files do not overlap):**
1. FLA-19 (Imports, small, finishes first)
2. FLA-21 with FLA-31 (acquisition chain, shim; the longest)
3. FLA-22 (search)
4. FLA-23 (Discover Weekly; starts with the trigger spike; only Program.cs line in common)
5. FLA-25 (updater tests; fully disjoint, lowest priority, can wait for a free agent)

Four large builds in parallel is the most I would run: the review load (QA plus Security on each
PR) is the real limit, not the build time. If review capacity is thin, start FLA-19, FLA-21+31 and
FLA-22, and start FLA-23 as soon as the first of them is merged.

**Step B:** FLA-18 after FLA-19 is merged and FLA-21's shim commit (commit 1) is in; then FLA-20
after FLA-19 and FLA-18. FLA-11 any time after FLA-28 merges.

**Step C:** FLA-26's scan jobs may start with Step A (question 14); the matrix run and FLA-27
wait for all builds.

**Review rule per PR:** QA verifies against its criteria; Security reviews FLA-18, FLA-19, FLA-21,
FLA-23 (user data, credentials, external input) and any FLA-20/22/25 change that adds a host, a port
or an admin route. A human merges, or the orchestrator on the human's standing instruction (D9).

---

## 4. Open questions for the human

Each has my recommendation. Where I could not verify something, it says so.

**1. Deezer as a second search source (FLA-22).** *Recommend: approve, with `SEARCH_FUSION_ENABLED`
on by default.* Deezer track search is not an existing call, but the public API is already used
by default for albums, artists and enrichment without a key, so this is more volume to a host
already in use, not a new service. Cost: one extra request per query of 12 songs or more, plus
fewer per-row lookups because track rows already carry album, cover and length. Octo's rate limiter
already caps Deezer at about 50 requests per 5 seconds. **Not verified:** Deezer's published terms of
use for the public API (I did not read them); please have one person look before approving, because
the QA note on FLA-13 asked for this. Default on is consistent with how albums already work and
fixes the "no Last.fm key means no outside songs" case, which is the biggest cause of poor search.
If you would rather start cautious, default off for one release and flip it.

**2. Last.fm: measure with a real key, or skip? (FLA-22)** *Recommend: skip it as a gate.* The
Deezer path does not need a key, and the unit tests use canned answers for both sources. Ask whoever
has a key to run the 15 queries once after the PR is merged and paste the counts on FLA-22, to
re-tune the fusion weights later. Waiting for it would stall the largest user-visible fix for a
measurement that only tunes it.

**3. `songCount` of 12 or less keeps skipping outside search? (FLA-22)** *Recommend: keep it.* It
protects both upstreams from a request per keystroke, and the fixture has a case that fails the
build if outside calls appear there. Revisit with measurements after the fusion is live.

**4. Discover Weekly: opt-out or opt-in? (FLA-23)** *Recommend: opt-out (every allowed user is
covered once the module is on), only if the user can opt out without an admin; otherwise opt-in.*
Security accepts opt-out under five conditions: switch off by default (kept), no outbound call
carries a user's data unless they linked a service themselves, the opt-out is reachable by the
user, the first list says what it does, and opting out stops runs and deletes their rows. The
condition most likely to fail is the third: this repo has no end-user app (the players live in
another repo). However `ImportsController` already serves signed-in non-admin Navidrome users
through the dashboard, so a small "Discover Weekly" panel there can carry the opt-out and the
explanation. If that panel is cut for time, the honest choice is opt-in, not an admin-only opt-out.

**5. Server-wide caps (FLA-23).** *Recommend: 100 outside songs per week for the whole server, 20
per hour, and stop fetching below 10 GB free.* These are my numbers, not measurements: with the
per-user cap of 10, 100 covers ten active users, which is a household or a few friends. They are
settings, so a larger server raises them. The design asks the human to confirm the defaults; the
per-user cap of 10 is already decided.

**6. API-key sign-ins get no Discover Weekly list in v1 (FLA-23).** *Recommend: accept.* A request
carrying an API key has no user name Octo can trust (Octo asks Navidrome who owns the key, but the
design refuses to start a run on a credential that carries `apiKey`/`jwt`, Security condition 15).
Status says why. Add a later issue only if someone actually uses an API-key client.

**7. Does the yt-dlp switch default on? (FLA-21)** *Recommend: no, keep it off for this release, and
file one follow-up issue now.* Security condition 11 forbids default-on until SEC-7 is addressed for
yt-dlp: a pinned, checksum-verified yt-dlp (or a setting that turns the runtime self-update off), and
the dependency scan in CI. Reason: the fallback makes yt-dlp run unattended, more often, on text
that comes from outside, and `-U` on a 403 downloads and runs an unverified binary as root today.
Proposed follow-up issue (Backend, small, no gate needed, can run in Step 0): "Pin and verify
yt-dlp in the shim and add a setting to turn off its self-update", with Security's review. Together
with FLA-26's scan jobs it clears condition 11, and the default then becomes a separate, one-line
human decision with Security's re-review. The switch's own README paragraph must say the risk until then.

**8. YouTube Music import scope (FLA-18).** *Recommend: v1 is public playlist links and a Takeout
file; no cookies, no OAuth, no stored login.* The architecture map (A3) says nothing reads a
YouTube Music account today. A "log in with your account" import means a stored cookie or token
that is a full-account credential, the worst class under R1.5, which would pull FLA-18 into a
credential-storage design plus Security's line-by-line review. The private liked-music list is
only reachable through Takeout in v1. If you want live account import, make it a separate issue
after v1, with its own design. **Not verified:** that Takeout's current file layout still matches
what the parser will assume; the build verifies it against a generated fixture and says so.

**9. Import fidelity (FLA-19): how do liked songs become stars?** *Recommend: pending stars applied
on the user's next live request, no stored credential* (FLA-19 section above). The alternatives are
worse: keep a token for a day (rejected for Discover Weekly for the same reason), or star as an admin
(forbidden by R2.5). Consequence the human should accept: liked songs are starred when the importing
user next opens a client, not at the moment of import, and only after the song is in the library.
Related, default: `IMPORT_STAR_LIKED` off until you have seen it work on your own server.

**10. FLA-22 thresholds (the gate must set them).** *Recommend as written in the FLA-22 section:*
12 of 13 scored fixture cases pass on canned answers, zero outside calls for type-ahead, both
scenarios pass; latency median plus 300 ms, p95 plus 500 ms, 10-second deadline unchanged. Reason
for 12 of 13: `Mr. Brightside live` is documented as needing two sources to agree; any other miss
is a real defect.

**11. Should the yt-dlp fallback decline songs that Discover Weekly fetches? (FLA-21 and FLA-23)**
*Recommend: yes.* The fallback is designed for songs a person asked for (hearts and imports through
`AcquireTrackAsync`). Discover Weekly also uses the acquisition entry points, so without a rule a
song Octo picked on its own that no source has could be taken as a YouTube MP3 at volume nobody
chose song by song. The rule is small: the Discover Weekly request marks its songs, and the
fallback declines marked songs. Without this, FLA-21 and FLA-23 interact by accident.

**12. A test-only stub service for the harness.** *Recommend: yes, one small Python (standard
library only) container in `tests/integration/docker-compose.yml` that stands in for Deezer, Last.fm,
slskd, Lidarr and the yt-dlp shim, with canned answers per module.* Four modules need stubs
(FLA-18 if the link path stays, FLA-21, FLA-22, optionally FLA-23), and `docs/testing.md` says a
module adds its own. A shared stub avoids four incompatible ones. `AGENTS.md` requires written
approval for a new container the plan does not list; this one exists only in the test stack,
never in the root `docker-compose.yml` or the images. Alternative: each module writes its own stub
file (more duplication, no approval needed).

**13. The remaining Medium and Low findings from the security audit.** *Recommend: file SEC-4/SEC-6
(secret files written with `0600` from the start) and SEC-5 (recovery code in the log) now as small
Backend issues; leave SEC-3, SEC-9 to SEC-13 as FLA-27 findings.* Only SEC-1 and SEC-2 were filed
(as FLA-28 and FLA-29). The audit's own text lists the Medium ones as "awaiting human approval" and
FLA-27's acceptance ("no open High or Critical; Medium and Low filed as issues") will make them
issues anyway, so filing the cheap, risk-reducing ones early costs nothing and the final review is
shorter. SEC-3 (dashboard on the main port) and SEC-11 (containers as root) change deployment and
need a human decision before anyone touches them.

**14. Split the dependency-scan jobs out of FLA-26.** *Recommend: yes.* They need no module, they
close SEC-7 (and Security condition 11), and the earlier a vulnerable dependency fails a build, the
cheaper it is. FLA-26 keeps the matrix run and the report.

**15. Anything else I found unresolved.**
- *FLA-10* (the page jumping while a song is fetched) was skipped by the human and sits in Todo with
  a draft PR. It is a Wave 0 item that FLA-26 lists as a dependency ("all Wave 0 UI fixes"). *Recommend:
  tell FLA-26 explicitly that FLA-10 is not required, or close FLA-10 as postponed.*
- *FLA-11* has a `Blocked` label although the PNG has been supplied; the comment says it waits for
  FLA-28 instead. The label should change to say that, or be removed when FLA-28 merges.
- *Per-user opt-out UI for Discover Weekly* and the *import options page* both add to the dashboard
  `admin.js`, which is one large file. Two agents editing it at once is a conflict risk: serialise
  FLA-20 and the Discover Weekly panel, or have each add a separate JS file under `wwwroot/admin`.
- *Discover Weekly's server-wide weekly cap* and *`PerHour`* of the yt-dlp fallback are separate
  budgets; nothing yet limits total unattended downloads from both together. *Recommend:* accept,
  document both in the README, and let FLA-27 check the sum.
- *CI evidence.* Decision D8 says CI runs. The docs-only PRs (FLA-13 to FLA-16) used `[skip ci]`
  and the earlier notes said Actions was off, so none of them has a CI run to cite. Before a build PR
  claims "CI passed", confirm `ci.yml` and `integration.yml` actually ran on that PR, and say so
  plainly when they did not.

---

## 5. Risks, and what to cut

### Risks

| Risk | Why it matters | Mitigation |
|---|---|---|
| The Discover Weekly trigger does not work as designed | Everything in FLA-23 hangs on a module-registered filter running after the port split without breaking body buffering (`Program.cs`); the design itself marks this as unverified | Do the spike as the very first FLA-23 task; the fallback is one claimed line in `SubSonicController.cs`; if both fail, drop FLA-23 to opt-in with a visible "Build now" button on the dashboard (the user's dashboard sign-in is also a verified request) |
| FLA-21 touches the most shared code (coordinator, shim, resolver, Soulseek, Lidarr, trickle) | Regressions in the heart-to-download path affect every user; the typed outcomes change exception types | Commits 1 and 2 first, behaviour-preserving, each tested; run the full unit suite and the harness after each; off means off is tested; Security reviews the shim change before the module |
| Four agents in parallel on shared hotspots | Rebase churn in `Program.cs`, `.env.example`, `docker-compose.yml`, `appsettings.json`, `README.md` | Merge in order; each branch rebases before review; per-module stub compose files; no reformatting |
| Review capacity (QA plus Security) is the bottleneck | Security must review four large PRs with long condition lists (21 and 19 conditions) | Stagger Step A; give Security the condition list as a checklist in each PR body |
| Upstream behaviour not measured | Last.fm unmeasured; Deezer volume and terms unverified; ListenBrainz `validate-token` unverified; Navidrome API details marked "verify" in several designs | Treat each as a build-time spike with a stated stop condition; do not guess in code |
| Harness cannot test time | yt-dlp fallback waits hours, Discover Weekly waits a week | Unit tests with a fake clock; harness pre-seeds valid store entries; say so in each PR instead of claiming a live test |
| Default-on pressure on the yt-dlp fallback | The human wants it useful; Security condition 11 says not yet | Keep the follow-up pin issue small and early (question 7) |
| Auto-downloading at volume (Discover Weekly, yt-dlp fallback) | Disk, abuse, terms | Per-user and server caps, free-space floor, switches off by default, both stated in the README |
| Data written to users' Navidrome accounts (stars, playlists) | A bug here changes users' real data | Off by default; written only with the user's own credential; A/B isolation tests; read back and verify |
| CI evidence | Past docs-only work had no CI run to cite | Confirm CI runs on this fork's PRs before Step A (question 15) and say plainly when it did not run |

### What I would cut if time is short

In this order, cheapest loss first:

1. **FLA-20** (the admin policy page). Everything works at today's defaults without it. The import
   options can still be set by switch.
2. **FLA-18's link path** (keep the Takeout file only). Removes the shim route and one stub.
3. **FLA-25** (updater verification). The updater already has shell tests; this is proof, not a
   feature.
4. **FLA-23's extras**: no ListenBrainz name map (use only users who have a token that
   proves a name), no seeded Last.fm similarity; library-only plus ListenBrainz recommendations.
5. Last resort: ship FLA-23 as opt-in with a dashboard button, which removes the trigger risk.

I would not cut: **FLA-19** (the brief asks for it and it is small), **FLA-21 with FLA-31** (the
human's explicit request, and it fixes a real outage bug), **FLA-22** (the most user-visible
quality fix), **FLA-26** (regression and scans), **FLA-27** (the only whole-system security pass).
