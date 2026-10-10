# Design: yt-dlp fallback for songs no source has

Written for FLA-14. It is the design FLA-21 ("Build the yt-dlp fallback") builds from, and it is
read by the Security agent before that. No code changes are made here. The Security sign-off
conditions for the build are in section 12; the human decisions of 2026-10-10 (Linear comment on
FLA-14) shape sections 1, 4, 5.4 and 9. Every claim about
today's behavior cites the file (and line) it was read from, on the commit this branch is cut
from (`0be6564`). Line numbers drift; search for the symbol if one is off.

Inputs: `docs/architecture.md` section 3.2 (the acquisition chain), `docs/modules.md` (what a
module is), `docs/security-requirements.md` (R4.1 to R4.6, SEC-8), and the README sections
"Spotify import" (`README.md:722`) and "Playback and acquisition" (`README.md:772`).

## 1. Summary

- Some songs, often remixes, are on YouTube and on no other source. Today the only way to get
  one is to put YouTube in the heart priority list, and then YouTube is tried **in its place in
  the list**. No legacy `DOWNLOAD_SOURCE` shape turns on YouTube and Lidarr together
  (`SubsonicSettings.cs:424-429`), so the order only matters for an explicit priority list,
  where it is the person's choice. With such a list a Soulseek outage can fall through to
  YouTube today (filed as FLA-31).
- The human's rule (2026-10-10): **Soulseek, and Lidarr if it is configured, are tried first.
  After several failed fetch attempts (not one miss), the song is fetched with yt-dlp and
  stored as MP3.** "Several" is a counted, stored number (section 4.2, default 3 attempts at
  least 6 hours apart). Lidarr failing, or Lidarr being unable to prove that nothing was
  grabbed, **never blocks the fallback forever**: it counts as a failed attempt. A Soulseek
  **outage** is still waited out, as the README promises (`README.md:728-730`, `:873`), and
  attempts made during an outage do not count.
- It is a **module** (`octo/Services/YtDlpFallback/`, switch `YTDLP_FALLBACK_ENABLED`) that
  plugs into the end of the existing chain through one small extension point. It reuses the
  existing YouTube download, identification, tagging and placement code; it adds the decision
  ("is this really a miss?"), the marking, and the retry-for-a-better-copy rule.
- The hard part is not the download. It is that **today no source can say "not found" and
  "I could not look" apart**. Section 3 lists the four places where an outage looks like a miss,
  and the small changes that make the signals trustworthy.
- The switch stays **off** by default. Whether it may default to on later is pending a human
  decision after more context on Security condition 11 (section 12). A song that is given up
  after 72 hours is shown on the dashboard, not dropped silently.

## 2. How the chain behaves today

From `docs/architecture.md` 3.2, rechecked against the code.

1. `HeartAcquisitionCoordinator.AcquireTrackAsync` (`octo/Services/Common/HeartAcquisitionCoordinator.cs:170`)
   first asks whether the library already has the song, then walks `EnabledSteps` (`:289`),
   which comes from `SubsonicSettings.EffectiveHeartDownloadSources()`
   (`octo/Models/Settings/SubsonicSettings.cs:395`). The first success ends the chain. A failure
   moves to the next step. The last failure marks the row failed (`:232`).
2. Heart, import trickle and (optionally) play all end in this one method
   (`TrickleWorker.cs` calls `hearts.AcquireTrackAsync`, `octo/Services/Imports/TrickleWorker.cs:83`).
3. **The legacy rows are built in the order Soulseek, YouTube, Lidarr**
   (`SubsonicSettings.cs:436-438`), but no legacy `DOWNLOAD_SOURCE` shape enables YouTube and
   Lidarr together (`SubsonicSettings.cs:424-429`). The default is Soulseek only
   (`SubsonicSettings.cs:387`). The ordering therefore only matters for an explicit priority
   list on the dashboard, where the order is the person's choice. The fallback does not touch it.
4. **Soulseek waits out an outage.** `HoldForSoulseekAsync` (`:368`) waits while slskd says it
   is not logged in, up to `Soulseek:OutageHoldHours` (default 6, `SoulseekSettings.cs:236`,
   clamped to 48 at `:283`), saved in `soulseek-holds.json` so a restart carries on.
   `SoulseekDroppedAsync` (`:399`) extends that when the login drops partway through a step.
   Two things limit it:
   - the wait is **skipped when Lidarr comes later** in the chain (`SoulseekDeadline`, `:358`),
     because Lidarr can still bring a lossless copy;
   - when the wait runs out, the chain **goes on** to the next source (the warning at `:385`).
5. The import trickle adds its own wait: `SoulseekOut` (`TrickleWorker.cs:88`) holds back a
   trickle song while Soulseek is not logged in, unless Lidarr is in the chain.
6. The YouTube step (`SoulseekDownloadService.DownloadViaYouTubeAsync`, `:301`) searches the
   shim for `artist title` (or uses the id already in the registry), has the shim write an MP3
   into `/music/.octo-incoming` (`:339`), verifies it with AcoustID and **keeps it even on a
   mismatch** (it goes to the Review playlist), because there is no second copy to try.
7. A lossy file is never final: **Better quality** lists every non-lossless song in the
   library, including YouTube ones (`README.md:875`), searches Soulseek and Lidarr for a
   lossless copy (never YouTube, `octo/Services/Library/UpgradeSources.cs:9`,
   `LibraryActionExecutor.cs:480`), and swaps it in place so plays, favorites and playlists are
   kept (`README.md:504`). The weekly auto-upgrade (`LIBRARY_ACTIONS_UPGRADE_PER_WEEK`, off by
   default, `README.md:514-520`) leaves a tried song alone for 28 days
   (`QualityUpgradeWorker.RetryAfter`, `QualityUpgradeWorker.cs:107`).

## 3. Why the signals cannot be trusted yet

The design needs "not found" to mean "the source looked and the song is not there". These are
the places in the current code where that is not what an exception or an empty answer means.

| # | Where | What it does today | Effect on the fallback |
|---|---|---|---|
| G1 | `SoulseekClient.StartSearchAsync` (`SoulseekClient.cs:446-459`) | If slskd refuses or fails to start a search, it logs a warning and returns `false`; `SearchWithEndAsync` then returns **no hits** (`:393-397`). | The caller sees an empty result and throws `No Soulseek ... found` (`SoulseekDownloadService.cs:782`). A failed search and a search that found nothing are the same exception. |
| G2 | `SoulseekClient.ReadServerAsync` (`:137-150`) and `ParseServerReading` (`:158`) | `null` (slskd unreachable, 401, timeout) and `Unknown` (state shape not understood) are **not** `NotLoggedIn`. | The hold only waits on `NotLoggedIn` (`HoldForSoulseekAsync` `:368`; `WaitForLoginAsync` returns on `null`/`Unknown`). A crashed slskd container does not hold; it falls through. Correct for the chain today (a broken slskd should not block hearts forever), wrong for a rule that says "never use YouTube while a source is down". |
| G3 | The chain's catch (`HeartAcquisitionCoordinator.cs:209`) | Any exception from a step, of any kind, is "this source failed" and moves on. `FileNotFoundException` is used for "nothing found" (`:782`), for "your picked copy was refused" (`:694`), for "Lidarr's file is gone" (`:234`) and for "YouTube download failed" (`:345`). | A type check on the exception cannot tell a miss from an error. |
| G4 | `LidarrHeartAcquisitionService.TryAcquireAsync` (`:583-608`) | Catches **every** exception, logs it, returns `false`. Lidarr unreachable, bad key, no album match and "grabbed but not imported in time" all return `false`. `ResolveAlbumAsync` throws a plain `InvalidOperationException` for "no match" (`LidarrClient.cs:102-109`). | The chain cannot tell "Lidarr does not have it" from "Lidarr is down". |
| G5 | `YouTubeResolver.SearchAsync` (`octo/Services/YouTube/YouTubeResolver.cs:67`) | Any non-success status is `null`. The shim answers **404 `no_hit`** for "YouTube has nothing" (`yt-dlp-shim/app.py:296`) and **502 `search_failed`** for "yt-dlp failed, timed out or the gate was full" (`:294`). | Octo cannot tell "no such video" from "the shim could not look". |

Also: `SoulseekDownloadService.cs:1163` throws a generic `Exception` when candidates **were**
found but every transfer or check failed. That is "found but not obtained", a third state that
must not count as a miss (a peer being offline tonight says nothing about whether the song exists).

## 4. Definitions

### 4.1 Per-source outcomes

Each source that takes part in a chain run reports exactly one of these. They replace the
"threw or did not throw" signal for the fallback's purposes; the existing chain keeps working
on exceptions as before (section 5.3).

| Outcome | Meaning | Counts as a miss? |
|---|---|---|
| `NotApplicable` | The source is not in this heart's enabled steps, or is not set up (`UpgradeSources.SoulseekSetUp` / `LidarrSetUp`, `UpgradeSources.cs:19-29`). | Ignored. It neither blocks nor satisfies the rule. |
| `Obtained` | A file landed. | Chain ends; no fallback. |
| `NotFound` | The source was healthy before and after, did a complete look, and had no acceptable candidate. Definitions per source below. | Yes. Counts as a failed fetch attempt. |
| `FoundNotObtained` | It had candidates but every transfer or check failed (`SoulseekDownloadService.cs:1163`), or Lidarr grabbed a release that has not imported. | Yes, for the attempt count: the fetch failed (a peer offline tonight is still a failed fetch). It does not by itself prove the song is absent. |
| `Unavailable` | The source was down, not logged in, unreachable, rate limited, or the look could not complete (G1, G2, G4, G5). | **Soulseek: no, the attempt is not counted** (outage, section 5.4). **Lidarr: yes, counted as a failed attempt** (it must not block forever). |

### 4.2 "Several failed fetch attempts"

The earlier draft required one run in which every source said `NotFound`. The human decided
that a single miss is not enough, but that Lidarr trouble must not stop the fallback forever. The
rule is now a count.

> A **fetch attempt** is one complete run of the chain for a song (every applicable source
> looked, `AcquireTrackAsync` reached the end without `Obtained`). It is **counted** when
> **Soulseek was verified up for the whole run** (or Soulseek is not applicable). It is **not
> counted** when Soulseek was `Unavailable` at any point (an outage): that run is parked and
> repeated (section 5.4). Lidarr never decides whether an attempt counts: its
> `NotFound`, `FoundNotObtained` and `Unavailable` (down, bad key, no album match, or "cannot
> prove nothing was grabbed") all count as a failed attempt.
>
> The song becomes **fallback eligible** when it has at least `FailedAttempts` counted attempts
> (default **3**) and each counted attempt was at least `AttemptSpacingHours` after the previous
> one (default **6**, so at least 12 hours from the first counted attempt to the third).

How it is counted and stored, per song (`ytdlp-fallback.json`, section 5.4):

| Field | Meaning |
|---|---|
| key | `provider` + `externalId` + `requestedBy` (per user, section 12 condition 9) |
| `firstMissUtc` | when the first run ended without a file; starts the `MaxWaitHours` clock |
| `countedAttempts` | number of counted attempts so far (an integer, not a list of raw text) |
| `lastCountedUtc` | time of the last counted attempt; the next one waits until `lastCountedUtc + AttemptSpacingHours` |
| `lastOutcomes` | the last run's outcome per source (enum names only), for the dashboard |
| `parkedReason` | `waiting-soulseek`, `waiting-spacing`, `rate-limit`, `shim-unavailable` |

The counting record is removed when the song is obtained (by any source, including a later
Soulseek hit), when the "already in the library / already yours" check finds the file
(`AlreadyYoursAsync`, `:173`) on a re-run, or when the song is given up (it then moves to the
given-up list, below). An attempt that ends in `Obtained` never counts. The worker owns the
counter; the coordinator only reports outcomes (section 5.3). With the defaults that is three
Soulseek/Lidarr tries, then yt-dlp.

**A lost or corrupt store fails toward not fetching.** If `ytdlp-fallback.json` is missing,
unreadable or corrupt, the module treats every song as `countedAttempts = 0`: nothing becomes
eligible from a store it cannot trust, so no yt-dlp download starts from a damaged file. A
single corrupt entry is skipped and logged (sanitised) without crashing the worker and without
dropping the other entries (and so without dropping other users' entries); a wholly unreadable
file is moved aside (kept `0600`, at most two kept, older ones deleted) and a new empty one is
started.

**What counts as a corrupt entry** (condition 20): a missing field, a count outside 0 to a sane
maximum, an unknown enum value, or an unparseable time. Such an entry is skipped and logged
(sanitised) and **never filled with defaults** (a missing time never becomes
`DateTime.MinValue`).

**Removal with the user and the song** (condition 17): a user's records (counting, parked,
given-up, and moved-aside files) are deleted when the user is deleted and when the song is
unhearted. A global size cap applies on top of the per-user caps, and the store is never part of
a backup, diagnostics or support download reachable by the admin or another user.

**Given-up list.** A song that is given up (72 h, or `NotFoundAnywhere`) is written to a
separate persisted list in the same store (`givenUp`), so the dashboard has something to show.
It holds only provider, externalId, `requestedBy`, `givenUpUtc`, `countedAttempts`, the
reason (enum) and the last outcomes (enum names). No titles, no free text. It is kept for
`GivenUpRetentionDays` (default 30, then pruned) and capped per user (`MaxGivenUpPerUser`,
default 100, oldest dropped). A new heart of the same song clears its given-up entry, but **never resets**
`countedAttempts`, `lastCountedUtc` or `firstMissUtc` of a song that already has a counting or
parked record: heart, unheart and heart again cannot bypass the spacing, and a run started
before `lastCountedUtc + AttemptSpacingHours` does not count (condition 21).

**Two users, one song.** The key includes `requestedBy`, so each user has a record. The
download is one at a time and starts with the "already in the library" check, so the second
user's request finds the file and does not download it again; when the file lands, the other
users' records for that song are removed.

**Clock.** All times are UTC wall-clock. A stored timestamp in the future is clamped to now when
read, and spacing and give-up are never computed from a future timestamp. The host clock is
trusted (no user can move it): a forward step of the host clock does shorten a wait, a step back
only delays. The store is written only when something changes (not on every 30 s poll), through
a temporary file created `0600` and moved over the target.

**Minimums.** The settings are clamped when read so the rejected "single miss" behaviour cannot
come back by configuration: `FailedAttempts` at least 2, `AttemptSpacingHours` at least 1,
`MaxWaitHours` at least `FailedAttempts * AttemptSpacingHours` plus 6 (24 at the defaults
3 x 6 + 6; the true floor is 8, from 2 x 1 + 6). Defaults are 3, 6 and 72. All other settings
have a documented range too (condition 16, below).

**Settings are validated on the server** (condition 15): they come only from env/appsettings or
`/api/admin`, are validated and clamped at read and at write (never in the page), and none takes
a URL, path or id. Clamp order, in 64-bit arithmetic: each setting to its own range first, then
`MaxWaitHours >= FailedAttempts * AttemptSpacingHours + 6` from the clamped values. Ranges
(minimum to maximum, default): `FailedAttempts` 2 to 10 (3); `AttemptSpacingHours` 1 to 72 (6);
`MaxWaitHours` computed minimum to 720 (72); `PerHour` 1 to 120 (12); `PerUserPerHour` 1 to
`PerHour` (4); `MaxParkedPerUser` 1 to 1000 (200); `MaxGivenUpPerUser` 1 to 1000 (100);
`GivenUpRetentionDays` 1 to 365 (30); `MinFreeGb` 1 to 1000 (5); `MaxMinutes` 1 to 60 (15);
`DurationTolerance` 1 to 60 (8). Zero, a negative or a non-numeric value is clamped to the
minimum or the default and **never means "no limit"**.

Zero applicable sources applies only to an **empty chain** (no enabled step at all, so no
YouTube row either): there is nothing to count and the fallback is eligible at once. A person
whose chain contains the YouTube row has chosen YouTube themselves and the fallback declines
(5.2).

"Applicable" means in the enabled steps for this kind of heart (`EnabledSteps(albumHeart:false)`,
`:289`) **and not switched off**. "Not applicable" means the step is **switched off**. A step
that is switched on but not set up (no slskd URL or key, or its settings cannot be read) counts
as `Unavailable`: the song parks and the run is not counted, so a misconfiguration never unlocks
yt-dlp. The empty-chain case also goes through `PerUserPerHour`, `PerHour` and `MinFreeGb`.

### 4.3 "A source is down"

> A source is **down** when, at any point in the run or in the final check, its health probe
> is not a clear "up", or one of its looks could not complete. A **Soulseek** outage parks the
> song and the run does not count. A **Lidarr** outage counts as a failed attempt.

The signals, per source:

**Soulseek (slskd)**

| Signal | Reading | Up / down |
|---|---|---|
| `ISoulseekLink.ReadAsync(fresh:true)` (`SoulseekLink.cs`), `Link == LoggedIn` | slskd says connected and logged in | up |
| `Link == NotLoggedIn` | Soulseek's server is out or slskd lost its login | **down** (today's hold) |
| reading is `null` (unreachable, 401, timeout) | slskd container down, wrong key (G2) | **down** (new: stricter than the hold, for the fallback only) |
| `Link == Unknown` | state shape not understood | **down** (cannot prove up) |
| a search start failed (G1) | the look did not happen | **down** for that look |
| a search ended by its ceiling with the cancel not confirmed (`SoulseekClient.cs:408`, "ceiling, cancel not confirmed") | the answers may be incomplete | **down** for that look |

`NotFound` for Soulseek requires **all** of:
1. a fresh `LoggedIn` reading **before** the first search and a fresh `LoggedIn` reading
   **after** the last one, with no login drop between (compare the reading's username and watch
   `SoulseekLink`'s change log);
2. every planned query (`PlannedQueries`, `SoulseekDownloadService.cs:677`) started and ran to
   an end (finished, or ceiling with a confirmed cancel);
3. a search that stopped at its hit limit with nothing usable was repeated once wider (already
   done, `:738`) and the wider one also found nothing usable;
4. no candidate was tried (`tried == 0`). The exact condition today is `ranked.Count == 0`
   (`:782`).

**Lidarr**

| Signal | Reading | Up / down |
|---|---|---|
| `LidarrClient.IsReachableAsync` (`LidarrClient.cs:57`) true before and after | HTTP answers with the key | up |
| it throws, times out, or answers 401/5xx | | **down** |
| `ResolveAlbumAsync` found no album (`:102-109`) | Lidarr's metadata does not know a release for this song | `NotFound`, if reachable |
| search command accepted, nothing grabbed and nothing imported within `ImportTimeoutSeconds` (default 1800), and Lidarr's queue and history for the album show no grab | indexers had nothing | `NotFound`, if reachable |
| something grabbed or queued, not yet imported | a download is in progress or stuck | `FoundNotObtained` |

Lidarr is the weakest signal, because it fetches whole albums and a remix will often have no
album at all, which is a legitimate `NotFound`. FLA-21 should check whether
`GetAlbumImportStateAsync` (`LidarrClient.cs:371`) and the queue/history endpoints can prove
"nothing grabbed". **If they cannot, Lidarr's answer is recorded as `Unavailable`, and that is
a failed attempt, not a block.** This replaces the earlier recommendation that an unavailable
Lidarr means the fallback never fires: Lidarr is optional, and the human decided it must not
hold a song back forever. Lidarr being down for good costs only the attempt count, not the
fallback.

**YouTube (the fallback's own source)**

| Signal | Reading |
|---|---|
| shim `/search` 404 `no_hit` (`app.py:296`) | **YouTube has no match.** The song is not found anywhere. End: mark failed (section 5.5). |
| shim `/search` 502 `search_failed` (`:294`), `/download` 502 `download_failed`, timeout, gate full | shim or YouTube **unavailable**. Park and retry later; this is not a miss. |
| search hit with a duration far from the song's | treated as no hit (section 6.2) |

### 4.4 Decision table

| Soulseek (this run) | Lidarr (this run) | Result |
|---|---|---|
| `NotFound` or `FoundNotObtained` | `NotFound`, `FoundNotObtained` or `Unavailable` | **Counted attempt.** If `countedAttempts >= FailedAttempts` and the spacing is met, **fallback eligible**; otherwise park until `lastCountedUtc + AttemptSpacingHours` and run the chain again. |
| `NotFound` or `FoundNotObtained` | `NotApplicable` | Same: counted attempt. |
| `NotApplicable` | any except `Obtained` | Counted attempt, same rule. |
| `NotApplicable` | `NotApplicable` | Fallback eligible at once, **only for an empty chain** (no step enabled). If a YouTube row is in the chain the fallback declines (5.2). |
| **`Unavailable`** (any reason) | anything | **Not counted. Park.** Re-run the whole chain when Soulseek is up. |
| `Obtained` or Lidarr `Obtained` | | Done; the record is removed. |

The case that matters most is Soulseek down with Lidarr configured: the hold is skipped because
Lidarr is later (`:358`), the Soulseek step fails fast with G1's empty result (a false
`NotFound` today), and Lidarr says no. Under this design the Soulseek outcome is `Unavailable`
(login not `LoggedIn`, section 4.3), so the run is **not counted** and the song waits. Only
Soulseek outages are waited out like this; a Lidarr failure just adds to the count.

## 5. Where it plugs in

### 5.1 A module, with a switch

Per `docs/modules.md`:

| Part | Value |
|---|---|
| Code | `octo/Services/YtDlpFallback/` (`YtDlpFallbackModule.cs`, `YtDlpFallbackSettings.cs`, `YtDlpFallbackService.cs`, `YtDlpFallbackStore.cs`, `YtDlpFallbackWorker.cs`) |
| Switch | `YTDLP_FALLBACK_ENABLED`, default `false` (decided, section 11). Four places: `.env.example`; `docker-compose.yml` `YtDlpFallback__Enabled: ${YTDLP_FALLBACK_ENABLED:-false}`; `appsettings.json` `"YtDlpFallback": { "Enabled": false }`; `YtDlpFallbackSettings.Enabled`. README paragraph. |
| Tests | `octo.Tests/YtDlpFallback/` |
| Registration | one line in `octo/Program.cs`: `builder.Services.AddYtDlpFallbackModule(builder.Configuration);` |
| Checked at run time | the service reads `IOptionsMonitor<YtDlpFallbackSettings>.CurrentValue.Enabled` when it runs. Registered always. |
| Off means off | With the switch off the module makes no shim calls, starts no worker work, writes no file (no store file, no tags), and every question it is asked answers "declined". The chain behaves as it does today. |

Other settings (all in `YtDlpFallbackSettings`, env names in the same four places):

| Setting | Env | Default | Purpose |
|---|---|---|---|
| `Enabled` | `YTDLP_FALLBACK_ENABLED` | `false` | the switch |
| `FailedAttempts` | `YTDLP_FALLBACK_FAILED_ATTEMPTS` | `3` | counted failed fetch attempts before yt-dlp is used (section 4.2); **minimum 2** |
| `AttemptSpacingHours` | `YTDLP_FALLBACK_ATTEMPT_SPACING_HOURS` | `6` | least time between two counted attempts of the same song; **minimum 1** |
| `GivenUpRetentionDays` | `YTDLP_FALLBACK_GIVEN_UP_RETENTION_DAYS` | `30` | how long a given-up song is listed before it is pruned |
| `MaxGivenUpPerUser` | `YTDLP_FALLBACK_MAX_GIVEN_UP_PER_USER` | `100` | given-up entries kept per user, oldest dropped |
| `MaxWaitHours` | `YTDLP_FALLBACK_MAX_WAIT_HOURS` | `72` | how long a parked song waits (outage, spacing, rate limit) before it is given up and shown on the dashboard (section 5.4); clamped to at least `FailedAttempts * AttemptSpacingHours + 6` |
| `PerHour` | `YTDLP_FALLBACK_PER_HOUR` | `12` | most fallback downloads started per hour, **globally**; the hard cap |
| `PerUserPerHour` | `YTDLP_FALLBACK_PER_USER_PER_HOUR` | `4` | most per signed-in user per hour (Security condition 7, 13) |
| `MaxParkedPerUser` | `YTDLP_FALLBACK_MAX_PARKED_PER_USER` | `200` | parked/counting songs per user; rows over it are dropped with a plain message |
| `MinFreeGb` | `YTDLP_FALLBACK_MIN_FREE_GB` | `5` | no download starts below this free space on the download volume (condition 6) |
| `DurationTolerance` | `YTDLP_FALLBACK_DURATION_TOLERANCE` | `8` (seconds) | search hit must be within this of the song's length (same as library matching, `README.md:867`) |
| `MaxMinutes` | `YTDLP_FALLBACK_MAX_MINUTES` | `15` (`10` when the registry says the song is under 6 minutes) | refuse longer videos (mixes, albums in one upload) |
| `RetryBetterDays` | `YTDLP_FALLBACK_RETRY_BETTER_DAYS` | `7` | how soon Better quality looks again for a fallback copy (section 7.3) |

### 5.2 Order, and its relation to the YouTube row

- The fallback runs **after** the last enabled step, including Lidarr. It is not a row in the
  heart priority list and is never reordered above Soulseek or Lidarr.
- The existing YouTube row stays as it is for people who chose it (legacy `SoulseekThenYouTube`
  and an explicit list). That is their stated wish; this design does not change defaults or
  existing behavior (out of scope per `AGENTS.md`, and a behavior change needs a human).
- With the module **on** and the YouTube row **also on**, YouTube has already been tried in its
  row; the fallback sees the chain contained a YouTube step (its `NotFound` or failure is
  recorded) and **declines**. The settings page shows a one-line note: "YouTube is already in
  your priority list; the fallback adds nothing while it is."
- The recommended setup is Soulseek, then Lidarr, with the YouTube row **off** and the fallback
  **on**. That is the brief's "YouTube last".
- Scope: **song hearts and import-trickle songs** (both enter through `AcquireTrackAsync`).
  Not album hearts (`AcquireAlbumAsync`), where one failing track is routine and a mass of
  YouTube MP3s would be surprising. Not play (`QueuePlay`): a played outside song already
  streams from YouTube. Albums can be added later as a separate switch.

### 5.3 The extension point

The coordinator is another feature's code, and `docs/modules.md` says a module talks to other
code "through an extension point it already offers". The coordinator has none, so the module
needs one small, claimed edit to `HeartAcquisitionCoordinator.cs` (not one of the `AGENTS.md`
hotspot files, but a shared service; the FLA-21 claim lists it and the PR calls it out):

```csharp
// octo/Services/Common/IAcquisitionFallback.cs (new, tiny, no dependency on the module)
public interface IAcquisitionFallback
{
    /// Asked once, after the last enabled step. Returns what it did.
    Task<FallbackResult> OfferAsync(ChainMiss miss, CancellationToken ct);
}
public sealed record ChainMiss(string Provider, string ExternalId, string? RequestedBy,
    DateTime? HeldSinceUtc, IReadOnlyList<StepOutcome> Steps);
public sealed record StepOutcome(HeartDownloadSource Source, SourceOutcome Outcome, string? Detail);
public enum SourceOutcome { NotApplicable, Obtained, NotFound, FoundNotObtained, Unavailable }
public enum FallbackResult { Declined, Obtained, Parked, NotFoundAnywhere }
```

The coordinator's changes, all inside `AcquireTrackAsync`:

1. Record a `StepOutcome` per step. Sources report it through **typed exceptions**, replacing
   G1 to G5 (this is the real work):
   - `SoulseekDownloadService` throws `SourceNothingFoundException : FileNotFoundException`
     **only** at the `ranked.Count == 0` site (`:782`) and only when the search health
     conditions of 4.3 held; a failed or incomplete search throws `SourceUnavailableException`
     instead. `SoulseekSearchOutcome` (returned by `SearchWithEndAsync`) gains a
     `Completed` flag so G1 and the unconfirmed cancel are visible. The all-attempts-failed
     throw (`:1163`) becomes `FoundNotObtainedException`.
   - `LidarrHeartAcquisitionService.TryAcquireAsync` keeps returning `false` (its callers rely
     on it) but also records an outcome on the job: `NotFound` / `FoundNotObtained` /
     `Unavailable` (4.3). `ResolveAlbumAsync` gets a typed "no match" result.
   - `YouTubeResolver.SearchAsync` returns a result carrying `NoHit` versus `Failed` (G5), by
     reading the status code it already has.
2. Any other exception type keeps today's meaning for the **chain** (move to the next step) but
   counts as `Unavailable` for the **fallback** (the safe direction).
3. After the loop, if the chain ended without `Obtained`, hand the recorded outcomes to the
   module, which counts the attempt (4.2), and call
   `_fallbacks.OfferAsync(...)` for each registered fallback, then fail the row as today if
   none returned `Obtained`. If a fallback returns `Parked` the row is **not** failed; it shows
   "Waiting for <source> to come back" using the tracker the same way the Soulseek hold does
   (`_tracker?.Stage(... AcquisitionState.Queued ...)`, `HeartAcquisitionCoordinator.cs:377`).
4. The constructor gains one optional parameter, `IEnumerable<IAcquisitionFallback>? fallbacks = null`
   (the existing constructor already takes optional services). With none registered nothing changes.

The module registers its `IAcquisitionFallback` in `AddYtDlpFallbackModule`, so Program.cs
gets exactly the one line. The module itself does not edit `SoulseekDownloadService` beyond
what 5.3 item 1 and section 7 list; those edits are a prerequisite, and FLA-21's first
commit(s) should be the typed outcomes (valuable on their own: they also fix the false
"No Soulseek found" row text during an outage), merged before the module.

### 5.4 Parking: outages wait, failed attempts are counted, then the *whole chain* runs again

When `OfferAsync` is called after a run that ended without a file, it does not try YouTube
unless 4.2 says the song is eligible. Otherwise it writes or updates a record in its own store
(`/app/config/ytdlp-fallback.json`, mode `0600` from the first byte, section 12 condition 9)
with the fields in 4.2 and returns `Parked`.

`YtDlpFallbackWorker` polls (30 s, like `SoulseekLink.PollEvery`; the store is written only on
change). For each parked song:

- `waiting-soulseek`: it checks that Soulseek now reads healthy (fresh) and then asks the
  coordinator to **run the chain again from the start** (`AcquireTrackAsync(..., heldSinceUtc)`),
  not to jump to YouTube. After an outage Soulseek may well have the song. This run is a
  normal attempt and counts if Soulseek stayed up.
- `waiting-spacing`: it re-runs the chain when `lastCountedUtc + AttemptSpacingHours` has passed.
- When a run ends and the count has reached `FailedAttempts`, the next step is the YouTube
  download (5.5), not another chain run.
- A parked song survives a restart (the store is read at start, like `SoulseekHoldStore`).

**Give up after `MaxWaitHours`** (default 72, from `firstMissUtc`; longer than the 6 hour
Soulseek hold on purpose). The row is failed with a plain text that says why ("Soulseek was
unavailable the whole time" or "only N of 3 attempts could be made"), no YouTube copy is taken
for an outage-only history, and the song is **listed on the dashboard** as given up (with its
count and last outcomes), not dropped silently. The person can heart it again, which starts a
new record.

- The import trickle needs no change: `SoulseekOut` already holds songs while Soulseek is not
  logged in. With Lidarr in the chain it does not hold (`TrickleWorker.cs:95`), and the chain's
  parking covers that case instead.
- Nothing is parked or counted when the module is off.
- **Who sees the lists (Security condition 18).** The dashboard's parked and given-up lists
  are filtered per user: a person sees only their own entries, and another person's entry
  answers as "not there". An admin sees **counts only** (how many parked, how many given up),
  not other users' songs, unless Security decides otherwise.

### 5.5 When the fallback itself runs

`OfferAsync` once the song is eligible (4.2):

0. **Fresh Soulseek check first.** Before the download starts, read Soulseek fresh again. If
   Soulseek is applicable and not `LoggedIn` (including `null` and `Unknown`), do not download:
   re-park as `waiting-soulseek`. An outage that begins after the count is reached wins.
1. Take the budgets: per user (`PerUserPerHour`) and global (`PerHour`); if either is
   exhausted, `Parked` with reason "rate limit" and retry next hour. Also refuse to start below
   `MinFreeGb` free space. Concurrency is one fallback download at a time (the shim also gates: the
   download runs as background work, `app.py:970`).
2. Call the existing `DownloadSongInternalAsync` pipeline with `sourceOverride = YouTube` and a
   new `FallbackProvenance` hint (section 6), through `TrackAcquisitionQueue.Enqueue`, the same
   call the chain makes (`HeartAcquisitionCoordinator.cs:201`). That reuses staging in
   `.octo-incoming`, AcoustID, tags, covers, lyrics, placement and the Navidrome rescan, and
   the "already mapped" and "already in the library" checks.
3. YouTube `no_hit` (4.3): return `NotFoundAnywhere`; the coordinator fails the row with the
   plain text "Not found on Soulseek, Lidarr or YouTube." and the dashboard lists it. Shim or
   download failure: `Parked` (retry, bounded by `MaxWaitHours`); these retries are not fetch
   attempts of the Soulseek/Lidarr chain and are not counted there.
4. Success: `Obtained`, the record is removed, the file is stored as MP3.

## 6. Tagging and marking a YouTube-sourced file

### 6.1 What is recorded, and where

Today a YouTube copy is recognized only by being non-FLAC: `BaseDownloadService.SourceLabel`
(`:124-125`) returns `YouTube` for any file that is not FLAC and has no `FetchedFrom` entry, so
it is a guess, and nothing records the video. The design adds an explicit, durable mark in
three places:

| Where | What | Why |
|---|---|---|
| **File tags** (TagLib, `TagWriterExtras` like `SetRecordingId`, `BaseDownloadService.cs:1585`) | custom frame `OCTO_SOURCE = youtube-fallback`; `OCTO_SOURCE_ID = <video id>` | Survives moving files, a rescan, and a lost `.mappings.json`. Lets a person or a script see what is a stopgap. Standard tags (artist, title, album, MusicBrainz ids) are written exactly as for any download. The values are constants plus the validated id only (no username, no `requestedBy`, no path, no timestamp). |
| **`LocalSongMapping`** (`LocalLibraryService.cs:390`) | two new optional fields: `Provenance` (`"youtube-fallback"`) and `SourceId` (video id), beside `SourcePeer`/`SourceFile` | Optional, so old `.mappings.json` entries load (the same pattern as `TranscodedFrom`). This is what Better quality and "Wrong song" read. |
| **`DownloadHistoryEntry`** (Fetched songs, `DownloadHistoryEntry.cs:20`) | `Source = "YouTube"` (unchanged) plus a `Fallback = true` flag and the reason text "Not on Soulseek or Lidarr" | The Fetched songs page and the download notification say why a lossy file arrived. |

`FetchedFrom` (`BaseDownloadService.cs:118`) already exists for "which backend" when the format
does not say; the fallback sets it, and FLA-21 should stop `SourceLabel` guessing.

The artist and title written come from the song's identity (the registry or the import), not
from the YouTube video title (R4.6; the shim retags with the clean values Octo passes,
`app.py:994-1010`). **YouTube-supplied free-text tags (comment, description, source URL,
uploader) are dropped**, with `-map_metadata -1` or an allowlist instead of
`-map_metadata 0`: only artist, album artist, title and the cover are set (Security condition 10).
The same constants-only rule applies to the `.mappings.json` fields.

### 6.2 Making sure it is the right video

A fallback exists for songs nobody else has, so a wrong match is the main quality risk and
AcoustID will often have no fingerprint for a remix. In order:

1. The shim search takes the song's duration (`/search?duration=`) and already ranks by length
   and skips live versions (`_search_with_hint`, `_LIVE`, `app.py:347-401`).
2. Octo accepts a hit only within `DurationTolerance` of the song's length and under
   `MaxMinutes`; otherwise it counts as `no_hit`.
3. After download, AcoustID verification runs as it does today (`:352-379`): a clear
   mismatch is **kept and sent to the Review playlist** (existing behavior, because there is no
   second copy); a file with no decodable audio is deleted and the attempt is a failure.
4. If a person marks the file "Wrong song", the video id is remembered per song and not
   picked again (the Soulseek blacklist mechanism, `RejectedPeerRegistry`, keyed by
   `youtube:<id>`; the entry holds only the song key and the id, never a username).

### 6.3 Not marked as owned music

The mapping and tags say "stopgap". They do **not** stop the file being a normal library song:
it plays, is favorited, and is playlisted like any other.

## 7. Does a later, better copy replace it?

**Yes, through the machinery that exists, not a new one.** The fallback file is lossy, so Better
quality already lists it (`QualityUpgradeWorker.cs:180`, `README.md:875`). What this design adds:

### 7.1 What counts as "better"

Unchanged from Better quality: a **lossless** copy from Soulseek or Lidarr that passes the same
identity and spectrum checks. A YouTube copy never replaces a YouTube copy, and the fallback
never runs for a song the library already has (`AlreadyYoursAsync`, `:173`), so there is no
loop. A different lossy copy (a 320 kbps MP3 from Soulseek) is not "better" by this rule; that
is the existing Better quality rule and is left alone.

### 7.2 How the swap goes

In place, as today (`README.md:504`): the file is replaced, the Navidrome entry keeps its plays,
favorites and playlist places. On success the mapping's `Provenance` and `SourceId` are cleared
and set from the new source (a Soulseek file sets `SourcePeer`), the tags of the new file are
those of a normal download (no `OCTO_SOURCE` frame), and the history gets a new entry. The old
YouTube file goes to the quarantine Better quality already uses.

### 7.3 When it looks again

Today a song that found nothing is not retried for 28 days (`QualityUpgradeWorker.cs:107`), and
the weekly auto-upgrade is off by default (`LIBRARY_ACTIONS_UPGRADE_PER_WEEK`, `README.md:514`).
A fallback copy exists precisely because the source did not have it that day; a peer coming
online makes it likely to be found within days. So:

- Songs with `Provenance == "youtube-fallback"` use `RetryBetterDays` (default 7) instead of
  28. The worker reads the mapping (`GetMappingsAsync`, `LocalLibraryService.cs:290`) to join on
  the path; FLA-21 should confirm that join does not slow the walk (`MaxPages` 200).
- Nothing starts looking by itself if the weekly setting is 0 (default): this design does not
  switch on the auto-upgrade. The dashboard's Better quality page and the apps' "Find higher
  quality" show these songs first (sorted by provenance) with a "YouTube copy" label.
  That the auto-upgrade is off by default is called out in the README paragraph for the switch.

### 7.4 What does not replace it

- A file the person put in the library themselves has no provenance and is never touched.
- A song replaced by a person (Wrong song, Wrong version) follows those actions' rules.
- If Soulseek is down when Better quality runs, the existing `AvailableAsync`
  (`UpgradeSources.cs:60`) leaves the song waiting; the fallback adds nothing.

## 8. Security notes (for the Security agent)

Public repository: this section states the controls and where they go, not how to defeat the
current gaps.

### 8.1 What crosses the Octo to shim boundary

The fallback adds **no new shim endpoint and no new parameter**. It uses the two calls the
YouTube step already makes:

| Call | Parameters | Origin of each |
|---|---|---|
| `GET /search` (`app.py:278`) | `q` = `"<artist> <title>"`; `duration` (int) | artist and title come from the song's identity: Last.fm/Deezer/YouTube Music results, or a **Spotify/CSV import row** (so attacker-influenced text, e.g. from a public playlist or a CSV a user uploads). Duration from the same row. |
| `GET /download` (`app.py:925`) | `id` (video id), `dest` (path without extension), `artist`, `title` | `id` from the shim's own search result or the registry, never from a request. `dest` built by Octo: `<DownloadPath>/.octo-incoming/<videoId>-<guid>` (`SoulseekDownloadService.cs:339`). `artist`/`title` from the song identity. |

### 8.2 Requirements for the fallback (mapped to `docs/security-requirements.md`)

| Id | Requirement | R-id / SEC | Where |
|---|---|---|---|
| F1 | A video id is validated against `^[A-Za-z0-9_-]{11}$` **in Octo before** it is used in a path (`:339`), a log, the registry, or a request; a non-matching id from the shim is treated as `no_hit`. | R4.2, SEC-8 | `YouTubeResolver`, `SoulseekDownloadService` |
| F2 | The same check **in the shim**, at the edge of `/download`, `/stream` and the URL builders (`app.py:806-807`, `:563-569`, `:931-953`), before the id reaches cache, log, URL or path. 400 on mismatch. This closes SEC-8 and must land before or with the module. | R4.2, SEC-8 | `yt-dlp-shim/app.py` |
| F3 | The `-o` output template: `dest` is canonicalised (`realpath`, as now), must be inside `<DOWNLOAD_ROOT>/.octo-incoming/` (tightened from "inside the root", `app.py:940`), and every `%` in it is escaped (`%` to `%%`) before it is placed in `-o f"{full_dest}.%(ext)s"` (`:952`), so no part of `dest` can be read by yt-dlp as a template field. Octo's own `dest` never contains `%`; the shim must not rely on that. The `.octo-incoming` directory is created by the shim or Octo and is never a symlink (condition 2). | R4.3, SEC-8 | `app.py` |
| F4 | Every argument is one `argv` list element, no shell (`_run` uses `subprocess.run(list)`, `app.py:205-235`; keep it). Free text goes only behind the fixed `ytsearch1:` prefix (`:316`), so it can never be read as an option. Nothing from a request becomes an option name or a yt-dlp/ffmpeg flag; no `--exec`, `--config-location`, `--proxy`, cookies or `-o` piece other than `dest` is ever accepted. The video URL is rebuilt by the shim from the validated id; a URL is never accepted from Octo, and `--` is placed before the URL in `/download` as defence in depth (condition 3). | R4.1, R4.12 | `app.py` |
| F5 | Text passed as `q`, `artist`, `title` is cleaned in **one place that every path uses**, `YouTubeResolver` (search, meta, download), not only the fallback module (control characters and newlines removed, length capped at 200). Import text (Spotify, CSV) is attacker input at that point; there is no route from import text to the shim that bypasses it, and there is a test per entry point: heart, import trickle, parked retry, Better quality retry. The shim independently rejects empty, over-long and control-character values with 400 for `q`, `artist`, `title`, `id`, and bounds `duration` (ASCII digits only, sane maximum). The `-metadata key=value` ffmpeg arguments are list elements with a fixed `key=` prefix (`:996-998`), so a value cannot start a new option. | R4.1, R4.6 | Octo, `app.py` |
| F6 | YouTube-supplied text (video title, channel, description) is never used as a path, command, or log format string. File names in the library come from the song's identity through `PathHelper` after identification, not from the video (R4.6). Log lines include the video id (validated) and song identity (sanitised), never raw YouTube text; this includes the existing lines in `YouTubeResolver.cs` (`:67`, `:85`) and the shim. | R4.6, R4.9 | pipeline |
| F7 | Limits: shim search 15 s and download 300 s timeouts (existing, `app.py:316-322`, `:970`), the background concurrency gate (existing); **new, enforced in the shim's `/download` itself** (the Octo-side check is not trusted; `--max-filesize` does not apply when the size is unknown and the mp3 re-encode can grow the output): `--match-filters` for `duration <= MaxMinutes*60 & !is_live`, `--max-filesize`, and a check of the final mp3 size that deletes the file and fails when over a cap; `.octo-incoming` is swept of partials after a failure; the module refuses to start below `MinFreeGb` free space; and its `PerHour`/`PerUserPerHour` and one-at-a-time limits. | R4.5 | `app.py`, Octo |
| F8 | The shim is compose-network only: `expose`, no `ports:` (`docker-compose.yml:328-329`). The module adds no port, no container, no endpoint, and no setting that lets a person type a URL or an id. The only trigger inputs are a signed-in person's heart or an import row they own. | R4.4, R3.8 | compose |
| F9 | The switch is flipped from the dashboard only through the existing `/api/admin` path (state change with `X-Octo-Admin`, admin session). The module adds no controller unless it needs a status read, and then under `/api/admin`. | R3.1, R3.2 | module |
| F10 | `ytdlp-fallback.json` holds usernames and ids of parked songs. It is written with a temporary file created `0600` and moved over the target, and it is removed entry by entry when a song is settled. No secret goes into it, the tags, the notification text, or logs. | R1.2, R1.4 | `YtDlpFallbackStore` |
| F11 | Per-user data: a parked entry stores its `requestedBy`; the dashboard's parked and given-up lists are filtered by the signed-in user taken from the session only, as `ImportStore.Get(owner, id)` does; **the admin read returns counts only** (no `requestedBy`, no provider id, no song), condition 18. A user cannot park, release or read another user's entry by id; "not yours" answers like "not there". The module has the A/B isolation test. | R2.2, R2.3, R2.7 | store, any endpoint |
| F12 | What leaves the box: the same as the existing YouTube step: artist, title and duration of a song to YouTube (via yt-dlp). No new third party. The README paragraph for the switch says so, and states the default. | R1.x, `README.md:319` | README |

### 8.3 Residual risks the Security agent should weigh

1. **SEC-7** (unpinned yt-dlp, runtime self-update) matters more once the module exists: the
   fallback increases how often yt-dlp runs on attacker-influenced search text. Not fixed here.
   It blocks default-on, not the build (Security condition 11, section 12).
2. **SEC-11** (shim runs as root with a read-write `/music`): a yt-dlp or ffmpeg parsing bug on
   a downloaded media file runs with that privilege. F3 and F7 narrow what can be written,
   not what runs. A non-root shim is the real fix.
3. **Import rows are attacker-influenced** (a public Spotify playlist or a CSV). The fallback is
   a new route from such a row to a YouTube search and a download. F1 to F7 and the per-hour
   caps are the controls; a shared playlist cannot cause more than `PerHour` downloads. `PerHour`
   is global *and* per user, so one user's large import cannot use up everyone's budget, and
   parked/counting entries have a per-user cap (`MaxParkedPerUser`) so a 10k-row import is not
   10k records.
4. **A wrong video kept as the song.** Mitigated by duration, length cap and AcoustID/Review
   (6.2), not eliminated.
5. **Rate limiting or blocking by YouTube** if the cap is set high. Default 12 an hour globally, 4 per user.

### 8.4 Tests the Security agent should expect (in `octo.Tests/YtDlpFallback/` and `yt-dlp-shim/tests`)

- F1/F2: ids of the wrong length, with `/`, `..`, `&`, `%`, `-`, spaces, unicode, and the empty
  string are refused at both edges; the shim answers 400 and nothing is cached or logged raw.
- F3: a `dest` with `%(...)s`, outside the root, outside `.octo-incoming`, a sibling-prefixed
  directory (`/music-evil`), and a symlink all refused or escaped; the output path is exactly
  the expected one.
- F4/F5: a query that starts with `-`, contains quotes, newlines, a URL, or `--` is passed only
  as the tail of `ytsearch1:` (assert the argv list).
- Off means off: with the switch off no shim call, no store file, no tag.
- Outage tests (section 9).

## 9. Preserving "wait out a Soulseek outage": the invariants

The module's tests encode these; a change that breaks one is a regression.

1. **A Soulseek outage is waited out and never counts.** If Soulseek is applicable and its
   fresh reading is not `LoggedIn` (including `null` and `Unknown`), at the start, at the end,
   or anywhere in between, the run is not a counted attempt and the fallback does not run.
   (Stricter than the hold on purpose, G2.)
2. **A hold timing out never becomes a fallback.** When `HoldForSoulseekAsync` gives up after
   `OutageHoldHours` and the chain goes on, the Soulseek outcome is `Unavailable`, so the run
   is parked and uncounted. Only a search that ran while logged in can count as an attempt.
3. **Lidarr being later does not hide an outage.** With Lidarr after Soulseek the hold is
   skipped (`:358`), but the Soulseek outcome is still `Unavailable`, so the run does not count
   (section 4.4, first rows).
4. **Lidarr trouble never blocks the fallback.** A Lidarr failure, or "cannot prove nothing was
   grabbed", counts as a failed attempt. A permanently broken Lidarr delays a song by the
   attempt count only.
5. **One miss is not enough.** The fallback needs `FailedAttempts` counted attempts spaced
   `AttemptSpacingHours` apart (4.2); the count is stored, survives a restart, and is cleared
   when the song is obtained or given up.
6. **The import trickle and the heart behave alike.** Both use the same entry and the same
   decision; the trickle's own `SoulseekOut` wait is untouched.
7. **After an outage the whole chain re-runs.** A parked song is re-offered to Soulseek first.
8. **Give-up is visible.** After `MaxWaitHours` the song is failed with a plain reason and listed
   on the dashboard.
9. **Existing behavior is unchanged with the switch off**, and for people who put YouTube in
   the list themselves.

Test cases (scripted `ISoulseekLink`, as the existing hold tests do): Soulseek `NotLoggedIn`
throughout with Lidarr set up and failing (nothing counted); three failed runs with Lidarr
down (counted, then yt-dlp); two failed runs then Soulseek has it (YouTube never called);
attempts closer than `AttemptSpacingHours` (not counted twice); give-up at 72 h with the
dashboard entry; counter survives restart; missing, corrupt and half-corrupt store (nothing eligible, other entries kept); two users, one song (one download, both records gone); future timestamp clamped; config below the minimums is clamped; Soulseek goes down after the count is reached (re-parked, no download); given-up entry is retained, pruned and capped; Soulseek `null` reading; Soulseek login drops mid
search; search start failure (G1); Soulseek has candidates that all fail transfer; eligible
and the shim `no_hit`; eligible and the shim 502; Lidarr unreachable; zero
applicable sources; YouTube row already on; restart while parked; parked song whose Soulseek
returns and finds it (YouTube never called).

## 10. Work breakdown for FLA-21

Suggested commits, each building:

1. `fix(shim)`: video id validation at the edge, `%` escaping, `dest` confined to
   `.octo-incoming`, `--` before the URL, input limits and the F7 limits in `/download`
   (F2 to F7; closes SEC-8), with shim tests. Independent of the module; lands before or with it.
2. `feat(acquisition)`: typed source outcomes (G1 to G5): `SoulseekSearchOutcome.Completed`,
   `SourceNothingFoundException` / `SourceUnavailableException` / `FoundNotObtainedException`,
   Lidarr outcome on the job, `YouTubeResolver` `NoHit` versus `Failed`. Existing chain behavior
   unchanged; tests assert the outcome for each case. (Also improves the failure text shown
   during an outage.)
3. `feat(acquisition)`: `IAcquisitionFallback`, `StepOutcome`, the post-loop call, the optional
   constructor parameter.
4. `feat(ytdlp-fallback)`: the module folder, switch (four places + README), decision logic
   (sections 4.2 and 4.4), attempt counting, store, worker, per-hour and per-user budgets,
   dashboard list of parked and given-up songs, tags and mapping fields, tests, one
   `Program.cs` line.
5. `feat(library)`: `RetryBetterDays` for fallback songs and the "YouTube copy" label.

Claims FLA-21 needs: `octo/Program.cs` (one line), `docker-compose.yml`, `.env.example`,
`octo/appsettings.json`, `README.md` (hotspots); `HeartAcquisitionCoordinator.cs`,
`SoulseekClient.cs`, `SoulseekDownloadService.cs`, `LidarrHeartAcquisitionService.cs`,
`YouTubeResolver.cs`, `LocalLibraryService.cs` (mapping fields), `QualityUpgradeWorker.cs`,
`yt-dlp-shim/app.py` (shared files, not hotspots). Security approval is required on the PR
(external input, shim, files).

## 11. Decisions and open questions for a human

Decided by the human on 2026-10-10 (Linear comment on FLA-14):

| # | Question | Decision |
|---|---|---|
| D1 | When does yt-dlp run? | Soulseek and (if configured) Lidarr first; after **several failed fetch attempts**, not one miss, yt-dlp fetches and stores MP3 (sections 4.2, 4.4). |
| D2 | Lidarr trouble? | Must not block the fallback forever; it counts as a failed attempt. This replaces the earlier "Lidarr unavailable means never fall back". |
| D3 | Soulseek outage? | Still waited out; attempts during an outage do not count. |
| D4 | Default of `YTDLP_FALLBACK_ENABLED`. | **Off** for now. Whether it defaults on later is pending, after the human has more context on Security condition 11. |
| D5 | Give up after `MaxWaitHours` (72)? | Yes, shown on the dashboard, not silent. |
| D6 | One small coordinator edit (the extension point)? | Accepted, as commits inside FLA-21 (section 10, items 2 and 3). |
| D7 | Album hearts? | Later, separate switch, separate issue. |
| D8 | The legacy order? | Unchanged. |
| D9 | The current outage fall-through with an explicit priority list | Filed separately as **FLA-31** (not part of this design). |

Still open:

| # | Question | Recommendation |
|---|---|---|
| Q1 | Default of `FailedAttempts` (3) and `AttemptSpacingHours` (6): are they the right balance between patience and a stopgap for remix-only songs? | Keep; configurable. |
| Q2 | Default-on later (D4): needs SEC-7 addressed for yt-dlp (section 12, condition 11) and a Security re-review. | A human decides after the condition is met. |
| Q3 | Keep a clear AcoustID mismatch (Review playlist, as today) or reject it? | Keep, as the existing YouTube step does, because remixes often fingerprint as something else. |
| Q4 | `PerHour` 12 global, `PerUserPerHour` 4, `MaxMinutes` 15 (10 for songs under 6 minutes). | Security suggested these (condition 13). |

## 12. Conditions for the build (Security, FLA-14 sign-off)

The Security agent approved the design with no Critical or High finding, subject to these 21
conditions (1 to 14 at the first sign-off, 15 to 21 added at the re-review of the attempt counting) for FLA-21 and its PR review (Linear comment on FLA-14, "[Security agent] Approved
(design)", 2026-10-10). The sections above have been changed where they conflicted.

1. **Video id**: F1 and F2 use exactly `^[A-Za-z0-9_-]{11}$` (full match, ASCII only) in Octo
   (`YouTubeResolver`, `SoulseekDownloadService`) and in the shim on `/download`, `/stream`,
   `/meta` and the URL builders, before any cache, log, path or URL use. The shim answers 400.
   Lands in the first (shim) commit, before or with the module.
2. **`dest`**: must resolve under `<root>/.octo-incoming/` (`realpath` plus `os.sep`, so
   `/music-evil` and symlinks fail); `%` is escaped to `%%` in the `-o` template; the shim does
   not rely on Octo's `dest`; `.octo-incoming` is created by the shim or Octo, never a symlink.
   Tests: symlinked `dest`, symlinked parent, sibling-prefix directory, `%(...)s` (F3).
3. **Arguments**: nothing but the fixed `ytsearch1:`/`ytsearch5:` prefix plus cleaned text is
   built from a request; a test asserts the argv for queries starting with `-` or `--`,
   containing a URL, another extractor prefix, quotes and newlines; no new yt-dlp flag from a
   request; `--` before the video URL in `/download` (F4).
4. **One place for query cleaning** (F5): Octo's `YouTubeResolver` (search, meta, download), not
   only the module; strips control characters and newlines, caps at 200; the shim rejects empty,
   over-long and control-character values with 400 and bounds `duration`; no route from import
   text bypasses it; **a test per entry point** (heart, import trickle, parked retry, Better
   quality retry).
5. **Sanitised logs** (F6): raw `q`, artist and title never appear in any log line on the
   fallback path, including the existing lines in `YouTubeResolver.cs` and the shim.
6. **Limits enforced in the shim** (F7): `--match-filters` for `duration <= MaxMinutes*60 &
   !is_live`, `--max-filesize`, and a final mp3 size check that deletes the file and fails;
   `.octo-incoming` swept of partials after failure; the module refuses to start below a
   free-space floor.
7. **Fairness**: `PerHour` applies **per user** and also globally (the global value stays the
   hard cap); parked entries have a per-user cap and the `MaxWaitHours` expiry; over the cap,
   rows are dropped with a plain message; a test shows a large import cannot exhaust another
   user's budget.
8. **No new surface** (F8, F9): no new port, endpoint, URL or id setting, or shim parameter; the
   switch is changed only via `/api/admin`.
9. **Store and per-user data** (F10, F11): the store file and its temporary file are `0600` from
   the start, outside the library and Soulseek-shared folders; the parked list and every release
   or read endpoint filter on the signed-in user (the admin sees counts only); "not yours"
   answers as "not there"; the A/B isolation test (R2.7); notification and history text are shown only to the
   requesting user.
10. **Tags and sharing**: `OCTO_SOURCE` and `OCTO_SOURCE_ID` carry only the constants and the
    validated id (never the username, `requestedBy`, host paths, timestamps or server ids), and
    so do the `.mappings.json` fields; the re-tag step drops YouTube-supplied free-text tags
    (comment, description, purl, uploader) with `-map_metadata -1` or an allowlist and sets only
    artist, album artist, title and cover; `.octo-incoming` stays out of the share list (R4.11),
    with a test; the "Wrong song" registry entry stores only the key and id.
11. **SEC-7 precondition for default-on**: FLA-21 may merge with the switch **off**. The
    default must not be switched on, and the README must not recommend it, until yt-dlp is
    **pinned and checksum-verified** (or the runtime self-update can be turned off) **and the
    dependency scan runs in CI**. The README paragraph for the switch says the fallback runs
    yt-dlp more often and that the self-update is a known risk until then. Security re-reviews
    when the default changes. (The human's decision D4 waits on this.)
12. **SEC-11 residual** accepted for now: a non-root shim is not required for this PR, but the
    PR comment links the SEC-11 issue; the F7 size limit is the only mitigation.
13. **Lowered defaults**: `PerHour` 12 plus a per-user share of 4; `MaxMinutes` 15, with 10 for
    songs the registry says are shorter than 6 minutes.
14. **Tests and review**: the F-series tests in 8.4, plus those above, are named in the PR with
    CI links (CI runs on PRs on this fork now; `docker.yml` stays disabled, so nothing is published);
    Security reviews the FLA-21 PR (external input, shim, files, per-user data).

Added by the Security re-review of the attempt counting, the persisted state and the settings
(Linear comment on FLA-14, "[Security agent] Changes requested (a992d36)", 2026-10-10):

15. **Settings source and validation**: all `YtDlpFallback` settings come only from
    env/appsettings or `/api/admin` (writes need the admin guard, F9). No setting takes a URL,
    path or id (condition 8 stays). Every value is validated and clamped on the server, at read
    and at write, never in the page.
16. **Bounds**, applied in this order with 64-bit arithmetic: clamp each setting to a documented
    range, then compute `MaxWaitHours >= FailedAttempts * AttemptSpacingHours + 6`. Minimum 1
    for `PerHour`, `PerUserPerHour`, `MaxParkedPerUser`, `MaxGivenUpPerUser`,
    `GivenUpRetentionDays` and `MinFreeGb`; 0 or a negative value never means "no limit"; a
    documented maximum for each, `MaxMinutes` included; `PerUserPerHour` never above `PerHour`;
    `FailedAttempts >= 2`, `AttemptSpacingHours >= 1`. A test per setting covers 0, negative,
    huge and non-numeric input. (Ranges in section 4.2.)
17. **Removal and retention**: a user's records (counting, parked, given-up, moved-aside files)
    are deleted when the user is deleted and when the song is unhearted. The moved-aside
    corrupt file is capped (one or two kept), kept `0600`, and covered by the same user-deletion
    rule. A global size cap exists on top of the per-user caps. The store is never in a backup,
    diagnostics or support download that the admin or another user can reach.
18. **Visibility**: the parked and given-up lists, the notification and history text, and any
    status read take the user from the authenticated session only (R2.1 to R2.3), never from a
    request parameter. The admin read returns **counts only**: no `requestedBy`, no provider id,
    no song. The A/B isolation test (R2.7) covers the parked list, the given-up list, the
    counts and the notifications, and shows a non-admin cannot read the admin counts.
19. **"Not applicable" means switched off**: a step that is on but not set up, or whose settings
    cannot be read, is `Unavailable` (park, not counted). A test covers an enabled Soulseek step
    with no slskd settings. The empty-chain "eligible at once" case also goes through
    `PerUserPerHour`, `PerHour` and `MinFreeGb`.
20. **Store entry validation**: an entry with a missing field, a count outside 0 to a sane
    maximum, an unknown enum value or an unparseable time is skipped and logged (sanitised),
    never filled with defaults. Tests cover each case.
21. **No reset by re-heart**: a new heart or retry of a song that already has a record never
    resets `countedAttempts`, `lastCountedUtc` or `firstMissUtc`. A run started before
    `lastCountedUtc + AttemptSpacingHours` does not count. A test covers heart, unheart and
    heart again in quick succession.
