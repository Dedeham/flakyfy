# Design: yt-dlp fallback for songs no source has

Written for FLA-14. It is the design FLA-21 ("Build the yt-dlp fallback") builds from, and it is
read by the Security agent before that. No code changes are made here. Every claim about
today's behavior cites the file (and line) it was read from, on the commit this branch is cut
from (`0be6564`). Line numbers drift; search for the symbol if one is off.

Inputs: `docs/architecture.md` section 3.2 (the acquisition chain), `docs/modules.md` (what a
module is), `docs/security-requirements.md` (R4.1 to R4.6, SEC-8), and the README sections
"Spotify import" (`README.md:722`) and "Playback and acquisition" (`README.md:772`).

## 1. Summary

- Some songs, often remixes, are on YouTube and on no other source. Today the only way to get
  one is to put YouTube in the heart priority list, and then YouTube is tried **in its place in
  the list**, before Lidarr in every legacy shape, and a Soulseek outage can fall through to it.
- The new rule: **YouTube is tried last, only when every source that could have had the song has
  positively said "not found", and never because a source was down.** A Soulseek outage keeps
  being waited out, exactly as the README promises (`README.md:728-730`, `:873`).
- It is a **module** (`octo/Services/YtDlpFallback/`, switch `YTDLP_FALLBACK_ENABLED`) that
  plugs into the end of the existing chain through one small extension point. It reuses the
  existing YouTube download, identification, tagging and placement code; it adds the decision
  ("is this really a miss?"), the marking, and the retry-for-a-better-copy rule.
- The hard part is not the download. It is that **today no source can say "not found" and
  "I could not look" apart**. Section 3 lists the four places where an outage looks like a miss,
  and the small changes that make the signals trustworthy.
- Default recommendation: switch **off** in the first release (a human decides, section 11).

## 2. How the chain behaves today

From `docs/architecture.md` 3.2, rechecked against the code.

1. `HeartAcquisitionCoordinator.AcquireTrackAsync` (`octo/Services/Common/HeartAcquisitionCoordinator.cs:170`)
   first asks whether the library already has the song, then walks `EnabledSteps` (`:289`),
   which comes from `SubsonicSettings.EffectiveHeartDownloadSources()`
   (`octo/Models/Settings/SubsonicSettings.cs:395`). The first success ends the chain. A failure
   moves to the next step. The last failure marks the row failed (`:232`).
2. Heart, import trickle and (optionally) play all end in this one method
   (`TrickleWorker.cs` calls `hearts.AcquireTrackAsync`, `octo/Services/Imports/TrickleWorker.cs:83`).
3. **The fixed order is Soulseek, YouTube, Lidarr** in every legacy `DOWNLOAD_SOURCE` shape
   (`SubsonicSettings.cs:424-429`, rows built at `:436-438`). The default is Soulseek only
   (`SubsonicSettings.cs:387`). So where YouTube is on at all, it is tried **before** Lidarr.
   The project brief wants it last. The dashboard list can reorder them, but a person has to
   know to do it.
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
| G3 | The chain's catch (`HeartAcquisitionCoordinator.cs:211`) | Any exception from a step, of any kind, is "this source failed" and moves on. `FileNotFoundException` is used for "nothing found" (`:782`), for "your picked copy was refused" (`:694`), for "Lidarr's file is gone" (`:234`) and for "YouTube download failed" (`:345`). | A type check on the exception cannot tell a miss from an error. |
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
| `NotFound` | The source was healthy before and after, did a complete look, and had no acceptable candidate. Definitions per source below. | **Yes. The only outcome that counts.** |
| `FoundNotObtained` | It had candidates but every transfer or check failed (`SoulseekDownloadService.cs:1163`), or Lidarr grabbed a release that has not imported. | No. Retry the chain later. |
| `Unavailable` | The source was down, not logged in, unreachable, rate limited, or the look could not complete (G1, G2, G4, G5). | No. Park and retry (section 5.4). |

### 4.2 "Not found anywhere"

> A song is **not found anywhere** when, in one run of the chain, **every applicable source
> returned `NotFound`**, and a final health check at the end of that run says every one of
> those sources is still healthy.

"Applicable" means in the enabled steps for this kind of heart (`EnabledSteps(albumHeart:false)`,
`:289`) and set up. Zero applicable sources is allowed (a person who turned off Soulseek and
Lidarr and wants only YouTube): nothing can be down, so the fallback is eligible at once. This
is the "YouTube only" shape, and it is the person's choice.

### 4.3 "A source is down"

> A source is **down** when, at any point in the run or in the final check, its health probe
> is not a clear "up", or one of its looks could not complete.

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
album at all, which is a legitimate `NotFound`. FLA-21 has to confirm that
`GetAlbumImportStateAsync` (`LidarrClient.cs:371`) and the queue/history endpoints can prove
"nothing grabbed"; **if they cannot, the rule is: Lidarr's answer is `Unavailable`, not
`NotFound`**, so the fallback errs toward not firing.

**YouTube (the fallback's own source)**

| Signal | Reading |
|---|---|
| shim `/search` 404 `no_hit` (`app.py:296`) | **YouTube has no match.** The song is "not found anywhere". End: mark failed (section 5.5). |
| shim `/search` 502 `search_failed` (`:294`), `/download` 502 `download_failed`, timeout, gate full | shim or YouTube **unavailable**. Park and retry later; this is not a miss. |
| search hit with a duration far from the song's | treated as no hit (section 6.2) |

### 4.4 Decision table

| Soulseek | Lidarr | Result |
|---|---|---|
| NotFound | NotFound | **Fallback eligible** |
| NotFound | NotApplicable | Fallback eligible |
| NotApplicable | NotFound | Fallback eligible |
| NotApplicable | NotApplicable | Fallback eligible (YouTube-only person) |
| Unavailable (any reason) | anything | **Park.** Re-run the whole chain when Soulseek is up. |
| NotFound | Unavailable | **Park.** Re-run when Lidarr is up. |
| FoundNotObtained | anything | Not a miss. Normal retry rules; no fallback. |
| Obtained | | Done |

The case that matters most is row 5 with Lidarr configured: Soulseek is down, **the hold is
skipped because Lidarr is later** (`:358`), the Soulseek step fails fast with G1's empty result
(a false `NotFound`), Lidarr says no, and today's chain would go to YouTube. Under this design
the Soulseek outcome is `Unavailable` (login not `LoggedIn`, section 4.3), so the result is
Park, not fallback.

## 5. Where it plugs in

### 5.1 A module, with a switch

Per `docs/modules.md`:

| Part | Value |
|---|---|
| Code | `octo/Services/YtDlpFallback/` (`YtDlpFallbackModule.cs`, `YtDlpFallbackSettings.cs`, `YtDlpFallbackService.cs`, `YtDlpFallbackStore.cs`, `YtDlpFallbackWorker.cs`) |
| Switch | `YTDLP_FALLBACK_ENABLED`, default `false` (recommended, section 11). Four places: `.env.example`; `docker-compose.yml` `YtDlpFallback__Enabled: ${YTDLP_FALLBACK_ENABLED:-false}`; `appsettings.json` `"YtDlpFallback": { "Enabled": false }`; `YtDlpFallbackSettings.Enabled`. README paragraph. |
| Tests | `octo.Tests/YtDlpFallback/` |
| Registration | one line in `octo/Program.cs`: `builder.Services.AddYtDlpFallbackModule(builder.Configuration);` |
| Checked at run time | the service reads `IOptionsMonitor<YtDlpFallbackSettings>.CurrentValue.Enabled` when it runs. Registered always. |
| Off means off | With the switch off the module makes no shim calls, starts no worker work, writes no file (no store file, no tags), and every question it is asked answers "declined". The chain behaves as it does today. |

Other settings (all in `YtDlpFallbackSettings`, env names in the same four places):

| Setting | Env | Default | Purpose |
|---|---|---|---|
| `Enabled` | `YTDLP_FALLBACK_ENABLED` | `false` | the switch |
| `MaxWaitHours` | `YTDLP_FALLBACK_MAX_WAIT_HOURS` | `72` | how long a parked song waits for sources to recover before it is given up (section 5.4) |
| `PerHour` | `YTDLP_FALLBACK_PER_HOUR` | `12` | most fallback downloads started per hour |
| `DurationTolerance` | `YTDLP_FALLBACK_DURATION_TOLERANCE` | `8` (seconds) | search hit must be within this of the song's length (same as library matching, `README.md:867`) |
| `MaxMinutes` | `YTDLP_FALLBACK_MAX_MINUTES` | `15` | refuse longer videos (mixes, albums in one upload) |
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
  **on**. That is the brief's "YouTube last", and it fixes the legacy "before Lidarr" order for
  anyone who switches.
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
3. After the loop, if the chain ended without `Obtained`, call
   `_fallbacks.OfferAsync(...)` for each registered fallback, then fail the row as today if
   none returned `Obtained`. If a fallback returns `Parked` the row is **not** failed; it shows
   "Waiting for <source> to come back" using the tracker the same way the Soulseek hold does
   (`_tracker?.Stage(... AcquisitionState.Queued ...)`, `HeartAcquisitionCoordinator.cs:372`).
4. The constructor gains one optional parameter, `IEnumerable<IAcquisitionFallback>? fallbacks = null`
   (the existing constructor already takes optional services). With none registered nothing changes.

The module registers its `IAcquisitionFallback` in `AddYtDlpFallbackModule`, so Program.cs
gets exactly the one line. The module itself does not edit `SoulseekDownloadService` beyond
what 5.3 item 1 and section 7 list; those edits are a prerequisite, and FLA-21's first
commit(s) should be the typed outcomes (valuable on their own: they also fix the false
"No Soulseek found" row text during an outage), merged before the module.

### 5.4 Parking: outages wait, then the *whole chain* runs again

When `OfferAsync` sees `Unavailable` (or `FoundNotObtained`), it does not try YouTube. It writes
a record to its own store (`/app/config/ytdlp-fallback.json`, created `0600` per R1.4 style
even though it holds no secret, because it holds usernames):

`{ provider, externalId, requestedBy, firstMissUtc, lastCheckUtc, reason }`

`YtDlpFallbackWorker` polls (30 s, like `SoulseekLink.PollEvery`). For each parked song it
checks that every source that was `Unavailable` now reads healthy (fresh), and then asks the
coordinator to **run the chain again from the start** (`AcquireTrackAsync(..., heldSinceUtc)`),
not to jump to YouTube. The reason: after an outage Soulseek may well have the song. The
fallback only fires when a complete run ends in 4.2's "not found anywhere".

- A parked song survives a restart (the store is read at start, like `SoulseekHoldStore`).
- It is given up after `MaxWaitHours` (default 72, longer than the 6 hour Soulseek hold on
  purpose: the hold is about not wasting a lossless chance on a short outage; this is about
  not taking a lossy copy at all). The row is failed with the plain text "A source was
  unavailable the whole time, so no YouTube copy was taken. Try again later."
- The import trickle needs no change: `SoulseekOut` already holds songs while Soulseek is not
  logged in. With Lidarr in the chain it does not hold (`TrickleWorker.cs:94`), and the chain's parking covers
  that case instead.
- Nothing is parked when the module is off.

### 5.5 When the fallback itself runs

`OfferAsync` with all outcomes `NotFound`/`NotApplicable`:

1. Take the per-hour budget (`PerHour`); if exhausted, `Parked` with reason "rate limit" and
   retry next hour. Concurrency is one fallback download at a time (the shim also gates: the
   download runs as background work, `app.py:970`).
2. Call the existing `DownloadSongInternalAsync` pipeline with `sourceOverride = YouTube` and a
   new `FallbackProvenance` hint (section 6), through `TrackAcquisitionQueue.Enqueue`, the same
   call the chain makes (`HeartAcquisitionCoordinator.cs:203`). That reuses staging in
   `.octo-incoming`, AcoustID, tags, covers, lyrics, placement and the Navidrome rescan, and
   the "already mapped" and "already in the library" checks.
3. YouTube `no_hit` (4.3): return `NotFoundAnywhere`; the coordinator fails the row with the
   plain text "Not found on Soulseek, Lidarr or YouTube." Shim or download failure: `Parked`
   (retry, bounded by `MaxWaitHours`).
4. Success: `Obtained`.

## 6. Tagging and marking a YouTube-sourced file

### 6.1 What is recorded, and where

Today a YouTube copy is recognized only by being non-FLAC: `BaseDownloadService.SourceLabel`
(`:124-125`) returns `YouTube` for any file that is not FLAC and has no `FetchedFrom` entry, so
it is a guess, and nothing records the video. The design adds an explicit, durable mark in
three places:

| Where | What | Why |
|---|---|---|
| **File tags** (TagLib, `TagWriterExtras` like `SetRecordingId`, `BaseDownloadService.cs:1585`) | custom frame `OCTO_SOURCE = youtube-fallback`; `OCTO_SOURCE_ID = <video id>` | Survives moving files, a rescan, and a lost `.mappings.json`. Lets a person or a script see what is a stopgap. Standard tags (artist, title, album, MusicBrainz ids) are written exactly as for any download. |
| **`LocalSongMapping`** (`LocalLibraryService.cs:390`) | two new optional fields: `Provenance` (`"youtube-fallback"`) and `SourceId` (video id), beside `SourcePeer`/`SourceFile` | Optional, so old `.mappings.json` entries load (the same pattern as `TranscodedFrom`). This is what Better quality and "Wrong song" read. |
| **`DownloadHistoryEntry`** (Fetched songs, `DownloadHistoryEntry.cs:20`) | `Source = "YouTube"` (unchanged) plus a `Fallback = true` flag and the reason text "Not on Soulseek or Lidarr" | The Fetched songs page and the download notification say why a lossy file arrived. |

`FetchedFrom` (`BaseDownloadService.cs:118`) already exists for "which backend" when the format
does not say; the fallback sets it, and FLA-21 should stop `SourceLabel` guessing.

The artist and title written come from the song's identity (the registry or the import), not
from the YouTube video title (R4.6; the shim retags with the clean values Octo passes,
`app.py:994-1010`).

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
   `youtube:<id>`).

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
| F3 | The `-o` output template: `dest` is canonicalised (`realpath`, as now), must be inside `<DOWNLOAD_ROOT>/.octo-incoming/` (tightened from "inside the root", `app.py:940`), and every `%` in it is escaped (`%` to `%%`) before it is placed in `-o f"{full_dest}.%(ext)s"` (`:952`), so no part of `dest` can be read by yt-dlp as a template field. Octo's own `dest` never contains `%`; the shim must not rely on that. | R4.3, SEC-8 | `app.py` |
| F4 | Every argument is one `argv` list element, no shell (`_run` uses `subprocess.run(list)`, `app.py:205-235`; keep it). Free text goes only behind the fixed `ytsearch1:` prefix (`:316`), so it can never be read as an option. Nothing from a request becomes an option name or a yt-dlp/ffmpeg flag; no `--exec`, `--config-location`, `--proxy`, cookies or `-o` piece other than `dest` is ever accepted. The video URL is rebuilt by the shim from the validated id; a URL is never accepted from Octo. | R4.1, R4.12 | `app.py` |
| F5 | Text passed as `q`, `artist`, `title` is cleaned by Octo (control characters and newlines removed, length capped at 200) and the shim rejects empty and over-long values. The `-metadata key=value` ffmpeg arguments are list elements with a fixed `key=` prefix (`:996-998`), so a value cannot start a new option. | R4.1, R4.6 | Octo, `app.py` |
| F6 | YouTube-supplied text (video title, channel, description) is never used as a path, command, or log format string. File names in the library come from the song's identity through `PathHelper` after identification, not from the video (R4.6). Log lines include the video id (validated) and song identity (sanitised), never raw YouTube text. | R4.6, R4.9 | pipeline |
| F7 | Limits: shim search 15 s and download 300 s timeouts (existing, `app.py:316-322`, `:970`), the background concurrency gate (existing); **new:** reject a hit over `MaxMinutes` before download and pass a hard `--max-filesize` cap to the download, so one bad upload cannot fill the disk; the module's `PerHour` and one-at-a-time limit. | R4.5 | Octo, `app.py` |
| F8 | The shim is compose-network only: `expose`, no `ports:` (`docker-compose.yml:328-329`). The module adds no port, no container, no endpoint, and no setting that lets a person type a URL or an id. The only trigger inputs are a signed-in person's heart or an import row they own. | R4.4, R3.8 | compose |
| F9 | The switch is flipped from the dashboard only through the existing `/api/admin` path (state change with `X-Octo-Admin`, admin session). The module adds no controller unless it needs a status read, and then under `/api/admin`. | R3.1, R3.2 | module |
| F10 | `ytdlp-fallback.json` holds usernames and ids of parked songs. It is written with a temporary file created `0600` and moved over the target, and it is removed entry by entry when a song is settled. No secret goes into it, the tags, the notification text, or logs. | R1.2, R1.4 | `YtDlpFallbackStore` |
| F11 | Per-user data: a parked entry stores its `requestedBy`; the dashboard's parked list is filtered by the viewer (an admin sees all), as `ImportStore.Get(owner, id)` does. A user cannot park, release or read another user's entry by id; "not yours" answers like "not there". The module has the A/B isolation test. | R2.2, R2.3, R2.7 | store, any endpoint |
| F12 | What leaves the box: the same as the existing YouTube step: artist, title and duration of a song to YouTube (via yt-dlp). No new third party. The README paragraph for the switch says so, and states the default. | R1.x, `README.md:319` | README |

### 8.3 Residual risks the Security agent should weigh

1. **SEC-7** (unpinned yt-dlp, runtime self-update) matters more once the module exists: the
   fallback increases how often yt-dlp runs on attacker-influenced search text. Not fixed here.
2. **SEC-11** (shim runs as root with a read-write `/music`): a yt-dlp or ffmpeg parsing bug on
   a downloaded media file runs with that privilege. F3 and F7 narrow what can be written,
   not what runs. A non-root shim is the real fix.
3. **Import rows are attacker-influenced** (a public Spotify playlist or a CSV). The fallback is
   a new route from such a row to a YouTube search and a download. F1 to F7 and the per-hour
   cap are the controls; a shared playlist cannot cause more than `PerHour` downloads.
4. **A wrong video kept as the song.** Mitigated by duration, length cap and AcoustID/Review
   (6.2), not eliminated.
5. **Rate limiting or blocking by YouTube** if the cap is set high. Default 12 an hour.

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

1. **No fallback while Soulseek is not verified up.** If Soulseek is applicable and its fresh
   reading is not `LoggedIn` (including `null` and `Unknown`), at the start, at the end, or
   anywhere in between, the fallback does not run. (Stricter than the hold on purpose, G2.)
2. **A hold timing out never becomes a fallback.** When `HoldForSoulseekAsync` gives up after
   `OutageHoldHours` and the chain goes on, the Soulseek outcome is `Unavailable`, so the
   result is Park. Only an actual search that ran while logged in can say `NotFound`.
3. **Lidarr being later does not hide an outage.** With Lidarr after Soulseek the hold is
   skipped (`:358`), but the Soulseek outcome is still `Unavailable`, so Lidarr may be tried
   and the fallback still waits (section 4.4, row 5).
4. **The import trickle and the heart behave alike.** Both use the same entry and the same
   decision; the trickle's own `SoulseekOut` wait is untouched.
5. **After an outage the whole chain re-runs.** A parked song is re-offered to Soulseek first.
6. **Existing behavior is unchanged with the switch off**, and for people who put YouTube in
   the list themselves.

Test cases (scripted `ISoulseekLink`, as the existing hold tests do): Soulseek `NotLoggedIn`
throughout with Lidarr set up and failing; Soulseek `null` reading; Soulseek login drops mid
search; search start failure (G1); Soulseek has candidates that all fail transfer; both
`NotFound` and the shim `no_hit`; both `NotFound` and the shim 502; Lidarr unreachable; zero
applicable sources; YouTube row already on; restart while parked; parked song whose Soulseek
returns and finds it (YouTube never called).

## 10. Work breakdown for FLA-21

Suggested commits, each building:

1. `fix(shim)`: video id validation at the edge, `%` escaping, `dest` confined to
   `.octo-incoming` (F2, F3; closes SEC-8), with shim tests. Independent of the module.
2. `feat(acquisition)`: typed source outcomes (G1 to G5): `SoulseekSearchOutcome.Completed`,
   `SourceNothingFoundException` / `SourceUnavailableException` / `FoundNotObtainedException`,
   Lidarr outcome on the job, `YouTubeResolver` `NoHit` versus `Failed`. Existing chain behavior
   unchanged; tests assert the outcome for each case. (Also improves the failure text shown
   during an outage.)
3. `feat(acquisition)`: `IAcquisitionFallback`, `StepOutcome`, the post-loop call, the optional
   constructor parameter.
4. `feat(ytdlp-fallback)`: the module folder, switch (four places + README), decision logic
   (section 4.4), store, worker, per-hour budget, tags and mapping fields, tests, one
   `Program.cs` line.
5. `feat(library)`: `RetryBetterDays` for fallback songs and the "YouTube copy" label.

Claims FLA-21 needs: `octo/Program.cs` (one line), `docker-compose.yml`, `.env.example`,
`octo/appsettings.json`, `README.md` (hotspots); `HeartAcquisitionCoordinator.cs`,
`SoulseekClient.cs`, `SoulseekDownloadService.cs`, `LidarrHeartAcquisitionService.cs`,
`YouTubeResolver.cs`, `LocalLibraryService.cs` (mapping fields), `QualityUpgradeWorker.cs`,
`yt-dlp-shim/app.py` (shared files, not hotspots). Security approval is required on the PR
(external input, shim, files).

## 11. Decisions and open questions for a human

| # | Question | Recommendation |
|---|---|---|
| Q1 | Default of `YTDLP_FALLBACK_ENABLED`. | **Off** in the first release (a lossy source, `docs/architecture.md` finding 5: defaults hide features). A later release can turn it on once it has run for a while. |
| Q2 | Give up a parked song after `MaxWaitHours` (72), or wait indefinitely? | Give up with a plain message; the person can heart again. Indefinite waiting hides a broken Soulseek. |
| Q3 | Treat "Lidarr cannot prove nothing was grabbed" as `Unavailable` (fallback never fires with such a Lidarr)? | Yes (4.3). Revisit after FLA-21 checks what Lidarr's API shows. |
| Q4 | Keep a clear AcoustID mismatch (Review playlist, as today) or reject it? | Keep, as the existing YouTube step does, because remixes often fingerprint as something else. |
| Q5 | Extend to album hearts later? | Separate switch, separate issue. |
| Q6 | Is one small edit to `HeartAcquisitionCoordinator` (the extension point) acceptable inside FLA-21, or should it be its own issue first? | Its own commits inside FLA-21 (items 2 and 3 of section 10), listed in the claim; they are the smallest change that keeps the module's boundary. |
| Q7 | Should the legacy `DOWNLOAD_SOURCE=SoulseekThenYouTube` shape, which takes YouTube before Lidarr, be changed? | No. Not in scope; it is the person's stated order. The settings page nudges to the fallback instead. |
| Q8 | `PerHour` default 12 and `MaxMinutes` 15. | Reasonable defaults; the Security agent may lower them. |
