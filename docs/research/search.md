# Research: why search returns so few results

Linear: FLA-13. Related build issue: FLA-22 (Improve search results). Fixture for the build task:
[`search-queries.json`](search-queries.json).

The brief says search works for an exact song or artist name but usually shows only the single
best match. This document finds out why from the code, shows it on 15 example queries, compares
three fixes and recommends one. Line numbers drift; search for the symbol if one is off.

## How this was checked

No live Octo server, no Last.fm key and no Navidrome were available. Three kinds of evidence are
used, and every claim below says which:

| Label | Meaning |
|---|---|
| **Executed (stub)** | Run in the dotnet 9 SDK container as a throwaway xunit test (deleted afterwards, not committed). It called the real `ExternalSearchService`, `SearchBudget`, `SearchSongOrder` and `SongIdentity` code with a stubbed Last.fm HTTP handler and a stubbed metadata service. What it proves is what Octo does with an upstream answer and what Octo asks for. It says nothing about what Last.fm really answers. |
| **Executed (live Deezer)** | One `GET https://api.deezer.com/search/track?q=<query>&limit=25` per query, one second apart, on 2026-10-10. Deezer's track search is public and needs no key. Counts and order will drift over time. |
| **Traced** | Read from code, not run. |

What could not be measured: what Last.fm's `track.search` returns for each query, and how
Navidrome's own search treats typos and partial words. Both are called **unmeasured** below and
are the first thing the build task should probe (see "Open questions").

## Where a search goes

`Search3` (`octo/Controllers/SubSonicController.cs:994`, also answers `search2`) sends the same
query two ways at once and puts the library rows first (`docs/architecture.md` section 3.1):

1. **The library:** Navidrome's own `search3`, with the query passed through as typed
   (`SubSonicController.cs:1110`). Octo does no matching of its own here.
2. **Outside songs ("discovery"):** `ExternalSearchService.GetAsync`
   (`octo/Services/Common/ExternalSearchService.cs:81`). It needs a Last.fm key (`:87`), asks
   Last.fm `track.search` once with the raw query (`:135`, URL built at
   `octo/Services/LastFm/LastFmService.cs:378`), cleans the answer (`LastFmSearchCleanup`),
   and, when fewer than 20 rows came back (`ThinSearchThreshold`, `:40`), pads with one
   `artist.gettoptracks` call (`:143-156`).

Two corrections to the starting points the issue names. `QueryVariantLookup` is not in `octo/`:
it is only a test class (`octo.Tests/QueryVariantLookupTests.cs`) for `SongIdentity.QueryVariants`
(`octo/Services/Common/SongIdentity.cs:1127`), which writes several spellings of a song whose
artist and title are already known and is used by lyrics and Deezer lookups
(`octo/Services/Metadata/DeezerMetadataService.cs:1115`, `octo/Services/Lyrics/KugouLyricsSource.cs:110`). Nothing in the search path calls it, so the user's
typed text is never widened. `SearchSongPagePlanner` only places later pages of a list
(`octo/Services/Subsonic/SearchSongPagePlanner.cs`); it cannot make a list shorter or longer,
and `SearchPagingTests` and `SearchSongPagePlannerTests` check only that paging neither repeats
nor skips rows.

## Root cause

There is no single defective line. Search is built on **one outside source asked one literal
question once**, and everything that could widen the answer is missing. Six findings, strongest
first.

### 1. The number of outside songs is whatever Last.fm's `track.search` returns

**Executed (stub).** With Last.fm answering N rows for a query, `GetAsync` returned N songs for
N = 0, 1, 5, 19, 20 and 50. Octo neither adds nor drops rows. So when Last.fm matches one row,
the user sees one outside row. There is no second source, no query rewriting (spelling, partial
words, "remix", artist/title order) and no ranking across the library and outside halves.
`docs/architecture.md` section 3.1 already noted this; the executed run confirms it.

### 2. Without a Last.fm key there are no outside songs at all

**Executed (stub).** No key: 0 songs and 0 calls to Last.fm, even though the stub would have
answered 50 rows. `README.md:51` says so: "without it search only finds your own music".
Albums and artists still come from Deezer without a key (`ExternalSearchService.cs:107`,
`SubSonicController.cs:1076`), so a search without a key can show albums and artists but
songs only from the library's own matches. This is the most likely cause of the
reported symptom for any install that skipped the optional key.

### 3. The fallback for thin answers asks about the wrong thing

**Executed (stub).** When `track.search` is empty, `anchor` falls back to the raw query
(`ExternalSearchService.cs:148`) and Octo asks Last.fm for the top tracks of an artist with that
name. The URLs sent, with the key removed:

```
?method=track.search&track=Bohemain Rhapsody&...&limit=50
?method=artist.gettoptracks&artist=Bohemain Rhapsody&...&limit=120
```

So a title-only, partial or misspelled query is looked up as an artist name. The call also sets no
`autocorrect` parameter (`LastFmService.cs:421`), which Last.fm's API offers for exactly
misspelled artist names (documented upstream behaviour, not tested here). The pad only helps a
correctly spelled artist that Last.fm already found (executed: 1 hit for "Daft Punk" plus 40
top tracks gave 41 songs).

### 4. Small `songCount` means no outside search; a full library leaves few slots

**Executed (stub), code in `SearchBudget.cs:66`.** The library keeps
`max(min(12, requested), requested / 4)` rows and discovery gets the rest:

| songCount | library target | outside target | outside rows on page one if the library returns 0 / 1 / a full set of matches |
|---|---|---|---|
| 5, 10 or 12 | the same number | 0 | none: the outside build is not started (`externalTask`, `SubSonicController.cs:1098`) |
| 15 | 12 | 3 | 15 / 14 / 3 |
| 20 (Subsonic default) | 12 | 8 | 20 / 19 / 8 |
| 50 | 12 | 38 | 50 / 49 / 38 |
| 100 | 25 | 60 | 60 / 60 / 60 |

The last column comes from `PageOneExternalCount`: library slots the library does not fill go to
outside songs, so a library with no match still gets a full page of outside rows at the default
count. A client that searches as the user types, with `songCount` of 12 or less, shows the library
only. This is by design (it keeps keystrokes cheap) and my judgement is that it is a likely
contributor for clients that never send a full search; it was not observed with a real client.

### 5. A newer or older keystroke cancels a running outside build

**Executed (stub).** `SupersedableBuildCoordinator` cancels an in-flight build for any query that
is a prefix of the new one or the other way round (`SupersedableBuildCoordinator.cs:49-52`).
With the enrichment step slowed to 3 seconds: `"daft"` first, then `"daft punk"` 300 ms later,
gave 0 songs for `"daft"` and 10 for `"daft punk"` (as intended). The reverse order, `"daft punk"`
first then `"daft"`, gave **0 songs for `"daft punk"`** and 10 for `"daft"`. A client that fires a
full query and a shorter probe together (for example a song search and an artist-name search), or a
user who backspaces, loses the longer query's outside rows to the cancel. The failure
is logged only at debug level (`ExternalSearchService.cs:99`). This is a smaller cause than 1 to
4 but it is silent.

### 6. Dedupe is exact; stylized and misspelled names do not merge

**Executed (stub), `SongIdentity.MatchKey`.**

| Artist and title | Key |
|---|---|
| Dua Lipa, Levitating | `dualipa\|levitating` |
| Dua Lipa, Levitating (Remix) | `dualipa\|levitating\|remix` |
| Dua Lipa, Levitating - Purple Disco Machine Remix | `dualipa\|levitating\|remix` |
| Dua Lipa, Levitating (Live) | `dualipa\|levitating\|live` |
| Dua Lipa, Levitating (feat. DaBaby) and Dua Lipa feat. DaBaby, Levitating | `dualipa\|levitating` |
| Queen, Bohemain Rhapsody (typo) | `queen\|bohemainrhapsody` (not equal to the correct spelling) |
| $uicideboy$, Kill Yourself | `uicideboy\|killyourself` (not equal to Suicideboys) |

Remixes and live takes are **kept apart**, so Octo does not hide remixes by merging them (a
suspicion worth ruling out). The two last rows show that a typo or a stylized name will not
dedupe against the library's spelling, which can list the same song twice once more outside rows
arrive. It is a quality issue to remember for the fix, not a cause of short lists.

### What is not the cause

- Paging: `SearchSongPagePlanner` and `SearchSongOrder` only place rows (both are covered by
  tests that pin no repeat and no skip). Rows are not lost between pages.
- Octo's dedupe of remixes and live versions (finding 6).
- The 60-row build size or the 10 second deadline: no executed case came near 60, and the
  deadline only matters when the upstream is slow (`ExternalSearchService.cs:36, 46`).
- Placeholder resolution: `SearchSongsByArtistTitleAsync` always returns exactly one song per
  input row (`SoulseekMetadataService.cs:61-110`), so it does not drop rows.

## Example queries

Fifteen queries are saved in [`search-queries.json`](search-queries.json) with the expected
behaviour for each. The "Deezer (live)" column is the number of rows Deezer returned for
`limit=25` (and its total) and what the first four rows were. Last.fm is **unmeasured** for every
row, so the column "Today" is what Octo is known to do with the query, not a measured result list.

| # | Query | Kind | What Octo sends or does today (basis) | Deezer (live) |
|---|---|---|---|---|
| 1 | `Blinding Lights` | exact title | Verbatim to `track.search`; thin answer pads with artist "Blinding Lights" (executed URL) | 25 of 219; The Weeknd first |
| 2 | `blinding lights the weeknd` | exact artist and title | Whole string as track name (executed URL) | 25 of 68; The Weeknd first |
| 3 | `$uicideboy$ kill yourself` | exact, stylized | Verbatim; key drops the `$` (executed) | 25 of 84; first four all `$uicideboy$` |
| 4 | `suicideboys kill yourself` | exact, spelled out | Verbatim; no alias on the query side (traced) | 25 of 84; first four all `$uicideboy$` |
| 5 | `bohemian rap` | partial title | Verbatim; pad asks for artist "bohemian rap" (executed URL) | 25 of 299; Queen's song first |
| 6 | `weeknd blinding` | partial, artist word | Verbatim as track name (executed URL) | 25 of 100; first four all The Weeknd |
| 7 | `blinding li` with songCount 5 | partial, type-ahead | No outside call at all (executed: external target 0) | not run |
| 8 | `Bohemain Rhapsody` | misspelled title | Verbatim; key differs from the real title (executed) | 25 of 300; Queen first |
| 9 | `dft punk` | misspelled artist | Pad asks for artist "dft punk", no autocorrect (executed URL) | 25 of 160; Daft Punk in rows 1 and 4, mixed with The Weeknd |
| 10 | `wekend` | misspelled artist (hard) | As 9 | 25 of 295; no Weeknd row in the first four |
| 11 | `daft punk` | artist only | `track.search` then pad with the first hit's artist (executed: 41 songs from 1 plus 40) | 25 of 175 |
| 12 | `Radiohead` | artist only, one word | As 11 | 25 of 205; first four all Radiohead |
| 13 | `Levitating remix` | remix, no artist | Verbatim; "remix" not read as a version (executed key shows remixes stay separate) | 25 of 127; first four all remixes |
| 14 | `Kavinsky Nightcall` | artist and title, versions | The case `LastFmSearchCleanup` exists for; versions stay separate (executed) | 22 of 22; Kavinsky first |
| 15 | `Mr. Brightside live` | live version | Verbatim; "live" is a title word (executed key) | 25 of 42; covers ahead of The Killers' live rows |

Reading across the rows: Deezer's keyless track search recovers a title typo (8), a partial title
(5), an artist-and-title string (2, 6) and returns many versions for a remix query (13). It does
not solve a one-word typo (10) and it mixes in the wrong artist for a typo (9). It also ranks
cover bands above the original for a "live" query (15). No single source is enough, which is why
the recommendation fuses sources.

## Options

All three keep Octo's contract that discovery is an addition to search, never a precondition:
a failed or slow outside source must still return the library's rows.

| | A. Fix Last.fm use in place | B. Add Deezer track search and fuse with Last.fm | C. Add YouTube Music search and fuse |
|---|---|---|---|
| What it does | Keep Last.fm as the only outside song source. Add the missing pieces around it: set `autocorrect=1` on the artist pad, anchor the pad on an artist found with `artist.search` rather than the raw query, retry a thin `track.search` with a variant (drop the last word, strip "remix" and "live", swap artist/title order). | Add `/search/track` on Deezer (public, keyless, already a dependency) as a second source, run in parallel with Last.fm, and merge by reciprocal-rank fusion with the existing `MatchKey` dedupe. Rows arrive with album, cover and length, so they need less per-row Deezer enrichment. | Add the shim's `/ytm/search?filter=songs` (`YouTubeMusicClient.SearchAsync`, `octo/Services/YouTube/YouTubeMusicClient.cs:30`) as a second source. Rows carry a `videoId`. |
| Result quality | Fixes artist typos (9) and thin multi-word queries. Title typos and remix queries depend on Last.fm's own tolerance (unmeasured). Still nothing without a key. | Fixes 2, 5, 6, 8, 13 on live evidence; 9 and 15 improve once the second source agrees with Last.fm. Works with no Last.fm key. 10 stays unsolved. | Likely strongest on typos and versions (Google's matcher), but not measured here. At most 20 rows per call (`yt-dlp-shim/app.py:761`). No ISRC. Works with no key. |
| Latency | +1 to 3 sequential Last.fm calls, but only when the first answer is thin (estimate: a few hundred ms each; not measured). | Runs in parallel with Last.fm, so wall time is the slower of the two. Live: 0.44 to 0.61 s per query from one machine. Cuts per-row Deezer lookups for rows Deezer already supplied. | Parallel, but the shim can answer `busy` (503, `app.py:768`). Expected to be slower than Deezer (estimate, not measured). |
| Upstream cost | Last.fm: up to 3 extra calls per thin query. Last.fm has no key-less path and rate limits per key. | Deezer: 1 extra request per query. Deezer already enforces about 50 requests per 5 seconds and the search path is the tightest user of it (`SoulseekMetadataService.cs:98-104`), so the saved per-row lookups must be checked to outweigh it in the build task. | YouTube Music through the shim: scraping risk, breaks when YouTube changes; no key. Adds load to the shim lane that also resolves streams. |
| Complexity | Low. Edits to `ExternalSearchService.BuildAsync` and `LastFmService`; no new source. | Medium. One new source class, one merge function, one switch. The merge is the new logic. | Medium to high. Shim availability handling, `busy` retries, no ISRC so weaker dedupe. |
| New third-party dependency | None | None (Deezer is already used) | None (already in the stack) |
| Main risk | Treats the symptom: still one source, still no key-less songs. | Deezer ordering is not always the original first (15). Needs the fusion to get ranking right. | Fragile upstream on the search path. |

## Recommendation

**Option B, with the cheap parts of A folded in.** It is the only option that fixes the key-less
case (finding 2), widens recall for four of the five problem kinds on live evidence, adds no
service and keeps latency at the slower of two parallel calls. Option A alone leaves one source
and no songs without a key. Option C is promising but depends on the shim's scraping path for the
most interactive feature and cannot be judged until it is measured; keep it as a later third
source behind its own switch if B leaves gaps.

### How it would be built

Follow `docs/modules.md`. A contained module, plus one call-site change in a file that is not a
hotspot.

| Part | Plan |
|---|---|
| Folder | `octo/Services/SearchFusion/`, namespace `Octo.Services.SearchFusion` |
| Switch | `SEARCH_FUSION_ENABLED`, default **true** (proposal; the build task should confirm). Four places: `.env.example`, `docker-compose.yml` (`SearchFusion__Enabled: ${SEARCH_FUSION_ENABLED:-true}`), `appsettings.json` (`"SearchFusion": { "Enabled": true }`), and `SearchFusionSettings.Enabled` with its default and env var named. Read at run time through `IOptionsMonitor`, not at registration. README paragraph. |
| Registration | One line in `octo/Program.cs`: `builder.Services.AddSearchFusionModule(builder.Configuration);` |
| Parts | `ISearchSongSource` (query in, ranked `(artist, title)` rows out), `LastFmSearchSource` (wraps today's `track.search` plus pad, with `autocorrect=1` and an `artist.search` anchor, and keeps `LastFmSearchCleanup`), `DeezerTrackSearchSource` (`/search/track`, limit about 25, through the existing Deezer HTTP client so `DeezerRateLimiter` applies), `SearchFusion` (reciprocal-rank fusion, dedupe on `SongIdentity.MatchKey`, with `LooseKey` as a second chance so `$uicideboy$` and `Suicideboys` merge). |
| Hook | `ExternalSearchService.BuildAsync` (`:128`) gains one branch: when fusion is on, collect rows from `SearchFusion` instead of lines `:135-156`; when off, run today's path unchanged. Everything after (placeholder resolution, enrichment, prewarm, single-flight, supersession) stays as it is. `SubSonicController`, `SearchBudget` and the paging classes are not touched. |
| Off means off | Switch off: no Deezer track call, no `artist.search`, no `autocorrect`; results are byte-for-byte today's. Tests cover both states. |
| Key-less | The Deezer source runs without a Last.fm key. The gate at `ExternalSearchService.cs:87` becomes "no source is available". This is a behaviour change to state in the README paragraph. |
| Tests | `octo.Tests/SearchFusion/`: the fusion function on the fixture with canned source answers; both sources failing returns the library only; the switch off equals today's output; no outside call at `songCount` under 12. `search-queries.json` is linked into the test output the way `song-identity-cases.json` is (`octo.Tests/octo.Tests.csproj:30`). |
| Shared files to claim | `octo/Program.cs` (one line), `.env.example`, `docker-compose.yml`, `octo/appsettings.json`, `README.md`. Also `octo/Services/Common/ExternalSearchService.cs` and one parameter in `LastFmService.cs` (not hotspots, but another feature's files: list them in the PR). |

Explicitly out of scope for the first build, as separate issues: the `songCount` under 12
type-ahead rule, library-side widening (a second Navidrome call with a relaxed query), and the
silent cancel in finding 5 (log it at information level at least).

### Acceptance for the build task

Run the fixture in two layers. Unit: canned Last.fm and Deezer answers per case checked against
`expect` (including the two scenarios). Live (manual, outside CI): each case against both
sources to confirm `minSongs` and `mustIncludeInTop`, since upstream answers drift.

## Open questions for a human

1. **Last.fm was never measured.** Before the build, someone with a key should run the 15
   queries against `track.search` and record counts and top rows, so the fusion is tuned against
   real Last.fm output. This changes how much of Option A is still worth building.
2. **Default for `SEARCH_FUSION_ENABLED`.** Proposed true. It changes behaviour for installs with
   no Last.fm key (they will now get outside songs). Is that wanted by default?
3. **Type-ahead.** Should `songCount` under 12 keep skipping outside search? Changing it raises
   Deezer and Last.fm load per keystroke (`SearchBudget.cs:21-28`). Left as is here.
4. **Deezer terms of use.** The public API is already used for albums, artists and enrichment.
   Adding a per-search track query raises volume; a human should confirm that is acceptable
   before the build.
