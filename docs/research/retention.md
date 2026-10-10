# Design: 30-day retention cleanup

Issue FLA-16. Written for the Security sign-off, and as the input to the build issue FLA-24.
This is a design. It changes no code and nothing in it has been run. The Security review of this
design is on the FLA-16 Linear issue (comment `[Security agent] Approved (design).`); its 19
conditions for the build are in section 11, and the body below has been changed where it conflicted
with them. `docs/modules.md`, which section 8 relies on, comes from FLA-8 and is not on `main` yet. Every claim about Octo cites
the file it was read from (line numbers drift; search for the symbol if one is off). Where a claim
is about Navidrome behaviour that this repository cannot prove, it is marked **verify** and
FLA-24 must check it against the pinned Navidrome in the FLA-12 harness before relying on it.

## 1. The goal, and the rule that decides every doubt

Remove songs Octo downloaded that nobody has played for 30 days and that are in nobody's playlist,
to save disk space.

**Rule of doubt: if Octo cannot prove, from complete and fresh data about every user, that nobody
wants a song, the song stays.** Missing data is never read as "nobody wants it". This single rule
is the reason for most of the design below: an empty answer and a failed answer must never look
alike.

## 2. The central problem: Octo cannot see what other users do

`docs/architecture.md` section 4.4 says it plainly: nothing records whose a file is. This section
works out what Octo can and cannot learn, and from which credential.

### 2.1 What Octo holds

| Credential | Where it comes from | What it can read | Reference |
|---|---|---|---|
| Navidrome admin identity | `Subsonic:AdminUsername` / `AdminPassword`, or captured when an admin signs in through Octo (memory only, lost on restart) | The admin's own plays, stars and ratings; every user's playlists and the user list through Navidrome's native API; the library with real file paths | `NavidromeIdentityService.cs` (`GetScanAuth` :85, `EnsureAdminJwtAsync` :265, `HasAdminIdentity` :286) |
| A listener's own sign-in | Present on each request that passes through Octo (`u` + token/salt, or an API key). Octo checks it with `rest/ping` (`CredentialCheck`). Never stored; the star-on-arrival hold keeps a token in memory for at most a day | That one user's plays, stars, ratings, playlists | `StarOnArrival.cs:13-16`, `RequestIdentity.cs` |
| Anything for a user who is not currently using Octo | none | nothing | |

### 2.2 What that means for each signal

- **Playlists: readable for all users.** `LibraryActionPlaylistWorker` lists every playlist with
  the admin token and reads each playlist's `ownerName` (`LibraryActionPlaylistWorker.cs:78-100,
  192-205`), and `NavidromePlaylistApi` says an admin may edit any user's playlist (`:11-14`).
  Reading private playlists of others as admin is therefore already how Octo works. This is
  checked at run time, not assumed. Each covered user's snapshot also records the ids of the
  playlists that user can see as themselves (`getPlaylists` with their verified credentials). The
  admin's playlist list must contain every one of those ids, and every owner it shows must be in
  Navidrome's user list (`/api/user`). If a playlist a user sees is missing from the admin's list,
  or an owner is unknown, the admin does not see everything and the run ends
  `skipped_playlist_visibility`. Also **verify** this on the pinned version in the FLA-12 harness
  (condition 18). The snapshot then stores playlist ids as well as song ids and dates.
- **Plays, favorites (hearts) and ratings: per user, in Navidrome, and not readable with the
  admin credential for anybody else.** Navidrome keeps them per user; Octo reads them "as the
  listener, so each song carries their own playCount, played (last played), starred and
  userRating" (`ForYouLists.cs:12-16`). The admin credential returns the admin's own values.
  **verify** that no Subsonic or native call lets an admin read another user's annotations.
- **Every user's request passes through Octo, but only the ones that use Octo.** Octo is the
  Subsonic endpoint clients talk to. A client pointed straight at Navidrome is invisible to it.

So for a user who is not signed in through Octo at the moment of a cleanup run, **Octo cannot read
their plays, hearts or ratings at all.** That is the gap. A design that "asks Navidrome about all
users with the admin account" would silently read only the admin's data and treat everybody else
as having played nothing. That is the failure this design exists to prevent.

### 2.3 Options for closing the gap

| # | Option | Covers | Cost and risk | Verdict |
|---|---|---|---|---|
| A | **Protection snapshots taken from live requests.** When a user's request passes through Octo with credentials Octo has just verified, a background step (rate-limited, at most once per user per day) uses that same request's credentials to read that user's starred items, ratings and last-played dates, and stores only the derived facts (song ids and dates, never the credential). | Every user who uses Octo at least once per snapshot window | No new secret stored, no new service, same pattern as `StarOnArrival` and Made for you (which already walks the library as the listener, `GeneratedPlaylistService.WalkLibraryAsync` :442). A user who does not appear for longer than the window is "uncovered" and handled by section 5.2. Direct-to-Navidrome plays are still seen, because the snapshot reads Navidrome's own record | **Recommended default** |
| B | Navidrome's database mounted read-only into Octo and read directly | All users, all history, no credentials | Security: gives Octo every user's password material and API keys, not just play data, and Octo runs as root (SEC-11). Also ties Octo to Navidrome's internal schema (**verify**), which can change between releases, and needs a live SQLite database with its WAL files opened read-only. Compose currently uses `deluan/navidrome:latest` (`docker-compose.yml:474`) | **Not recommended (Security).** Needs written human approval |
| C | Each user hands Octo a long-lived Navidrome API key | Everyone who does it | Security: the highest-risk class under R1.5, because the key is full access and long-lived. R1.5 allows tokens a user hands Octo if stored apart from `settings.json`, keyed by user, `0600`, removed on disconnect, but a full-access key held at rest is the most sensitive thing Octo would keep | **Not recommended (Security).** Needs written human approval |
| D | A play ledger kept by Octo from the plays it proxies (`rest/scrobble`, `rest/stream`) | Plays through Octo only, from the day it starts | History is empty at first, so alone it can never show "no plays for 30 days" | Used only as a cheap extra check (section 4.4), not as the source |

**Decision for the design: Option A, with the extra checks from section 4.** Security found A
acceptable and the right default (it stores no secret; a snapshot holds only song ids, dates and
flags). B and C are recorded as not recommended and need written human approval; nothing below
depends on them. Snapshot rules (keying, files, lifetime) are conditions 13, 14 and 16.

### 2.4 What happens for users whose credentials Octo does not hold

A user is **covered** if Octo has a snapshot of theirs no older than `SnapshotMaxAgeDays`
(default 14) at the time of the run. Otherwise they are **uncovered**.

- Their **playlists are still protected**, because admin reads all playlists (2.2).
- Their **plays, hearts and ratings are unknown**, so they protect nothing and the run cannot
  claim to have looked at them.
- Rule: **a run that finds any uncovered user does nothing at all** (state `skipped_uncovered`),
  and the report names the users. This is what keeps a quiet account (a user on holiday, or one
  who plays through a client that bypasses Octo) from losing songs.
- An admin may list a user in `Retention:IgnoredUsers` to say "this account is inactive, do not
  wait for them". That removes only the plays/hearts/ratings protection for that user, and each
  such user is printed at the top of every report. Their playlists still protect. A new user
  starts uncovered, so adding an account always pauses cleanup until they have used Octo.
- Consequence to state honestly: on a server where some user listens only through a client that
  bypasses Octo, cleanup will never run until that user is ignored or fixed. That is the safe
  failure.

## 3. Scope: what the job may touch at all

Only files that Octo itself downloaded: those with an entry in `.mappings.json`
(`LocalSongMapping`: `LocalPath`, `LocalSubsonicId`, `DownloadedAt`, `LocalLibraryService.cs:390-`).
The user's own rips and purchases, and anything Octo has no record of, are never candidates. The
genre backfill uses the same split (`GenreBackfillScope.OctoDownloads`, `GenreBackfillState.cs:11`)
and calls whole-library work "gated harder"; retention has no whole-library mode in this design.

Also out of scope, always: anything in the trash folder, `.octo-incoming`, and any file whose
`StorageMode` is `Cache` (those have their own hourly cleanup, `CacheCleanupService`).

## 4. The eligibility rule

A song is a **candidate** only if every condition below holds. Any single "no", or any "cannot
tell", keeps the song. `T` is the time of the run and `W` is `Retention:Days` (30; values below 30
are raised to 30 so a typo cannot shorten the window).

| # | Condition | Source | Note |
|---|---|---|---|
| E1 | The file has a mapping in `.mappings.json` and Navidrome resolves its id to the same full path | `LocalLibraryService`, `NavidromeSongPathResolver` native leg | See 4.2 |
| E2 | Age: `T` minus the latest of the mapping's `DownloadedAt`, the file's last write time and the newest journal entry for it is at least `W` | mapping, file system, `LibraryActionJournal` | A song hearted yesterday and not yet played is protected by this alone. A Better quality swap resets the clock |
| E3 | **Last play, every user.** For every Navidrome user, the user's last played time for the song (`played`) is missing or at least `W` before `T`. A time that cannot be parsed, is in the future, or has **no UTC offset** counts as recent | one snapshot per user (2.3 A) | "Missing" is only trusted for a covered user. All times are UTC. `ForYouLists.Played` (`:170`) parses with `RoundtripKind` and then calls `ToUniversalTime`, which treats a value without an offset as local time; retention must not reuse it as it is, and must reject such a value instead |
| E4 | **Favorites/hearts, every user.** Not starred by any user as a song, and neither the song's album nor its artist is starred by any user | snapshots: `getStarred2` returns songs, albums and artists | An album heart downloads the whole album (`architecture.md` section 3), so an album or artist heart protects every song of it. Conservative on purpose |
| E5 | **Ratings, every user.** No user has rated the song above 0 | snapshots (`userRating`) | See open question 1: a 1-star rating is also the "Delete" command when rating actions are on (`LibraryActionSettings.cs`, `Defaults`), but those act through their own worker; retention does not interpret it |
| E6 | **Playlists, all users.** In no Navidrome playlist of any owner, public or private, including Octo's own action and notice playlists | admin native `/api/playlist` and every playlist's tracks | Complete read required (4.1). Playlists that Octo serves without writing to Navidrome (Made for you and the genre/decade mixes, `generated-playlists.json`) also protect the songs currently in them |
| E7 | In no share and not in anyone's now-playing | admin `shares`, `getNowPlaying` (**verify** the admin sees all users') | A shared link must not go dead |
| E8 | **Keep answers.** The song is not in the notice queue as Kept for any user | `NoticeQueue` (`NoticeState.Kept`, `NoticeQueue.cs:9, 219`) | "Keep" is an explicit "I want this" |
| E9 | **Lidarr heart protection** (4.3) | | |
| E10 | No acquisition, hold or library action is in progress for it | `AcquisitionTracker`, `SoulseekHoldStore`, `LibraryActionExecutor._busy` (`:82`), journal `Pending` | An outage hold is a heart waiting to land (`SoulseekHoldStore.cs:19`) |

### 4.1 Reading Navidrome completely

Octo's existing readers are not safe for this as they are, and the design must not reuse them
unchanged:

- `NavidromePlaylistApi.ListPlaylistsAsync` returns an **empty list on any failure** and reads at
  most 1000 playlists (`:39-47`). `ListTracksAsync` also returns empty on failure and reads at most
  500 tracks of a playlist (`:57-58`). Used for retention, a failed call, or a long playlist,
  would make songs look unprotected.
- `ReadAllTracksAsync` (`:197`) is the right shape: pages through, and returns `null` for a failure
  "never an empty list". Retention needs the same for playlists and for the user list
  (`/api/user` is read with a 1000 cap at `:155`).
- `GeneratedPlaylistService.WalkLibraryAsync` stops silently after `MaxWalkPages` = 80 pages of
  500, so 40 000 songs (`:50, 442-470`). A snapshot must treat reaching the cap as failure, not as
  a short library.

Required of the retention reader: every list is read to the end; the number of rows read is
compared with the total Navidrome reports (**verify** the native API's total-count header); any
failure, timeout, short read, or cap makes the whole run `aborted_incomplete`. No partial results
are used.

### 4.2 Getting from a song to one file

Delete decisions rest on an id Octo resolved, never on text (R5.4). Use only the strict native leg
of `NavidromeSongPathResolver` (`TryNativeAsync`, `:93`): Navidrome's own library-relative path
plus `libraryPath`, then `Verify` (`:264`), which requires the path to be inside the music
root, the file to exist, the byte size to equal what Navidrome reports, and the suffix to match.
Additional requirements for retention:

- Refuse the Subsonic leg and the local-mappings leg. Navidrome's Subsonic `path` can be built
  from tags and name a different file (resolver comment, `:56-62`), and the mappings leg has no
  size check (`:237`).
- Refuse a reported size of 0 (`Verify` skips the size comparison when `candidate.Size` is 0,
  `:277`).
- The resolved full path must equal the mapping's `LocalPath` as a full path. If they differ, the
  song is skipped and the report says why.

### 4.3 The existing Lidarr heart protection

There is no single piece of code with this name. What exists, and what retention must honour:

1. **In-flight claims.** `LidarrAlbumClaims.HeartBusy` and `UpgradeBusy` (`LidarrAlbumClaims.cs`)
   mark albums Lidarr is working on. A song of a busy album is skipped.
2. **Files Lidarr manages.** `LidarrTrackFetcher` leaves "a file Lidarr itself manages" to Lidarr
   (`ManagedText`, `:49-50`, `:112-116`), and a heart through Lidarr never deletes files Lidarr
   already had (`LidarrHeartAcquisitionService.cs:276, 524-545`). Removing a file from under a
   monitored Lidarr album can also make Lidarr download it again. So: when Lidarr is configured,
   each candidate is looked up in Lidarr by path/album, and a song that has a track file there is
   skipped. If Lidarr is configured but cannot be asked, **that candidate** is skipped (not the
   whole run, since Lidarr only matters for its own files).
3. **A heart that is still arriving.** A song with a pending heart (an `AcquisitionTracker` row,
   a `SoulseekHoldStore` hold, a Lidarr job) is skipped (E10).
4. Hearts themselves are E4. After a Lidarr heart lands, the user's star is set by
   `StarOnArrival` if Octo held their credential; **if it was not, the star was never set**
   (`architecture.md` A7). That is why E2 (age since download) exists: a heart that never became a
   Navidrome star gets a full `W` to be played, and the dashboard report should show it.

### 4.4 Rechecks, so a run does not act on stale facts

- **Dwell:** a song is moved only if it was a candidate in an earlier evaluation at least 24 hours
  before (kept in the module's own state file). One bad read cannot remove anything.
- **Recheck at move time:** immediately before each move, E1 and E6 are read again for that song,
  and the play ledger (Option D: last proxied play/star/playlist-change time per song, from this
  server's own traffic) is consulted. Any activity since the evaluation cancels the move.
- **Fresh snapshots before a move (conditions 6 and 7).** A snapshot may be up to 14 days old
  when a song first becomes a candidate, and a play, heart or rating made after it, through a
  client that bypasses Octo, would be seen by nothing but the trash. To close that window: a song
  is moved only if **every covered user's snapshot was taken after the song's first evaluation as
  a candidate** (after the 24 h dwell began). A covered user without such a newer snapshot means
  the song waits. At move time the file's size, last write time and (where available) file id are
  captured again and compared with the evaluation, under the same per-song busy lock the library
  actions use (`LibraryActionExecutor._busy`, `:82`) so a download or action cannot touch the same
  file; any change cancels the move.
- **Clock sanity (conditions 8 and 9).** The state file keeps the last run's time. The run is
  skipped (`skipped_clock`) if `T` is earlier than the last run, if it is more than 3 days after it
  on the daily schedule, or if `T` differs from Navidrome's reported server time by more than a few
  minutes. This matters because E2/E3 use the local clock, and because `Sweep` decides what is old
  by comparing a dated folder name with `UtcNow` (`LibraryActionQuarantine.cs:223-233`): a clock
  that jumps forward would make everything look old and sweep the trash early.

## 5. When to do nothing

The whole run does nothing, removes nothing, and records why, in each of these cases:

| Case | Run state |
|---|---|
| `Retention:Enabled` is off | `off` (no outgoing call, no file written, per `docs/modules.md`) |
| No admin identity (`HasAdminIdentity` false). After a restart with only a captured login, this is the normal state until an admin signs in | `skipped_no_admin` |
| Navidrome unreachable, a request fails or times out, or the admin token is refused after one re-login | `aborted_unreachable` |
| Any list was read incompletely (4.1) | `aborted_incomplete` |
| Any user uncovered and not ignored (2.4) | `skipped_uncovered` |
| **Play history missing**: a covered user's snapshot has no play dates for any song at all while the library has songs; or the oldest snapshot data is younger than `W` (Octo cannot yet say "30 days with no play"); or Navidrome reports a scan in progress | `skipped_no_history` |
| Library looks wrong: the music root is missing or empty, Navidrome's song count is zero or unreadable, or it dropped more than 20 % since the last run (no division by zero) | `aborted_library_changed` |
| The clock check of section 4.4 fails | `skipped_clock` |
| The playlist owners seen do not match the user list (section 2.2) | `skipped_playlist_visibility` |
| Navidrome's version differs from the one the verify items were checked against (condition 18) | `aborted_version` |
| A cap would be exceeded (section 7) | `aborted_cap` |
| The journal or the module's state file cannot be written | `aborted_no_journal` (the file must not move until the record is on disk, as `LibraryActionExecutor` does, `:236-247`) |

"Do nothing" is the default branch: the code paths that remove are reached only when the run
state was reached by passing every row above.

## 6. Reusing the quarantine, and how long it lasts

Retention never deletes. It moves a verified file into the trash with `LibraryActionQuarantine.Move`
(`LibraryActionQuarantine.cs:96`), which keeps the folder layout under `.octo-trash/<yyyy-MM-dd>/`,
writes a manifest beside the file, and moves the song's own lyrics with it (`withSidecars: true`),
as the existing Delete action does (`LibraryActionExecutor.cs:253-272`). `Restore` (`:172`) puts a
file back, only inside the music root.

**How long:** the same window as every other trashed file, `LibraryActions:QuarantineRetentionDays`
(default 30, `LibraryActionSettings.cs`). `Sweep` (`:215`) is the only code that really deletes,
by the age of the dated folder, and `0` means never sweep. At defaults a song is therefore
unplayed for 30 days, then 30 more in the trash before its bytes are freed: **about 60 days from
last play to disk space back**, and 30 days during which an admin can put it back.

Three things about the existing flow that the build must handle, because they would make
retention either useless or harmful:

1. **The sweep only runs when Library actions are on.** `Sweep` is called from
   `LibraryActionPlaylistWorker.SweepAsync` (`:106`), which returns early unless
   `LibraryActions:Enabled` and `PlaylistsEnabled` are on (`:80-81`) and an action playlist is
   wanted. With Library actions off (the default), retention would fill the trash and never free
   space. The retention module calls `Sweep` itself on its own schedule, and its docs say the
   trash window is shared. That sweep applies the same clock check as the run, never runs in dry
   run or with the retention switch off, skips any dated folder that is a link, and does not
   follow links (condition 9).
2. **A retention removal must not block re-download.** A Delete action's journal entry makes the
   song "not downloaded again" (`LibraryActionEntry.BlocksDownloads`, `LibraryActionJournal.cs:54`).
   A song removed for being unplayed is not something the user rejected: if somebody hearts it
   later it must download again. The retention entries use a new `LibraryAction` value, appended
   at the end of the enum (the journal stores numbers, `LibraryActionSettings.cs`, the comment on
   `LibraryAction`), and `BlocksDownloads` must be false for it (it is true only for `Delete`,
   `LibraryActionJournal.cs:54-55`). **`LibraryActionJournal.Resolve` (`:237-280`) must handle the
   new value explicitly.** Today every non-Delete action is treated as "a replacement was being
   fetched": a Pending entry whose file is in the trash goes down that branch and calls
   `restoreOriginal`, putting the file back. That is the safe direction but the wrong logic. For
   retention: Pending with the file in the trash completes as Applied; Pending with the source
   still present completes as Failed, with no restore. Both are tested (condition 11). Restore from the dashboard's
   trash list (`LibraryEditService.RestoreAsync` :191) currently matches only `Delete` entries,
   so restoring a retention entry needs a small, admin-only addition there (R3.3).
3. **Navidrome only forgets the song at the next scan.** After a move, request a rescan the way
   the actions do (`LibraryRescan.Soon`, `LibraryEditService.cs:185-209`), and drop the mapping
   (`ForgetMappingAsync`, kept in the manifest so a restore re-links it).

Shared journal limit: the journal keeps 2000 entries (`LibraryActionJournal.cs:74`, trimmed by
`Trim`), so it cannot be the only record of a bulk removal. The per-run report and each trash
manifest are the durable record: the manifest is written before the journal entry is completed,
and reports are kept for the full trash window (condition 12).

## 7. Dry run, caps, and the report

- **Dry run is the default:** `Retention:DryRun = true`. A dry run does everything except the move
  and the rescan, including the full readings, the checks, the dwell bookkeeping and the report.
  It is the same stance as `LibraryActionSettings.DryRun` ("a rehearsal the operator reads before
  it is real"). Going live is a separate setting, and per `AGENTS.md` running it without dry-run
  on real data needs written human approval.
- **Caps per run (R5.3):** at most `MaxPerRun` songs (default 25) and at most
  `MaxLibraryPercent` (default 5 %) of the library. If more are eligible, the run **aborts**
  (`aborted_cap`) rather than taking the first 25: a sudden large count means the data is wrong,
  not that the library is suddenly full of dead songs. The cap is a ceiling, not a throttle.
  Hard-coded upper bounds apply whatever is configured. On top of that there is a **rolling
  cap**, default at most 100 songs in 7 days, also with a hard upper bound, because the per-run cap
  alone would let a consistent data error remove 25 songs every day (condition 10).
- **Report**, one JSON file per run (`retention-report-<timestamp>.json` under the config
  folder, last 30 kept) and a dashboard view. It contains:
  - run state, dry-run flag, times, the Navidrome and music-root identity, counts;
  - the covered, uncovered and ignored users and each snapshot's age;
  - **per candidate:** song, artist, album, full path, size, age, last play per user (date only),
    and the result of E1-E10 as "passed"; and whether it will be moved in this run or is waiting
    for its dwell day;
  - **per protected song (summarised):** the reason it stayed (`played by <user> <date>`,
    `starred by <user>`, `album starred`, `rated`, `in playlist <name> of <user>`, `shared`,
    `kept`, `lidarr`, `too new`, `pending acquisition`), so an admin can see the rule working
    and notice a reason that looks wrong;
  - everything that stopped the run, in words.
  The report shows an Octo admin per-user listening data, which an admin cannot otherwise read
  from Navidrome. So: per song it shows the reason class and the users' names, and only the date
  of the most recent play per user; the endpoint is admin-only and answers `Cache-Control:
  no-store` (R3.3, R3.6); the file is created `0600`; reports older than 30 days are deleted; and
  the README says that the report shows listening dates to an Octo admin (conditions 13, 14).

## 8. Module boundary (per `docs/modules.md`)

| Part | Content |
|---|---|
| Folder | `octo/Services/Retention/`: `RetentionModule.cs` (the extension method), `RetentionSettings.cs`, `RetentionWorker` (hosted service, runs once a day at a configured hour, checks the switch at run time), `RetentionEvaluator` (pure: the table in section 4), `NavidromeReader` (strict readers, 4.1), `UserSnapshotStore`, `RetentionReportStore`, `RetentionState` (dwell) |
| Switch | `RETENTION_ENABLED`, default `false`, in the four places: `.env.example`, `docker-compose.yml` (`Retention__Enabled: ${RETENTION_ENABLED:-false}`), `appsettings.json` `"Retention": { "Enabled": false }`, `RetentionSettings.Enabled = false` |
| Other settings (same four places) | `RETENTION_DRY_RUN` true, `RETENTION_DAYS` 30, `RETENTION_MAX_PER_RUN` 25, `RETENTION_MAX_LIBRARY_PERCENT` 5, `RETENTION_SNAPSHOT_MAX_AGE_DAYS` 14, `RETENTION_IGNORED_USERS` empty, `RETENTION_RUN_HOUR` 4 |
| Registration | one line in `Program.cs`: `builder.Services.AddRetentionModule(builder.Configuration);` |
| Controller | `octo/Controllers/RetentionController.cs`, under `/api/admin/retention`: read the latest report, run a dry-run now, apply. All admin-session only; the apply action refuses unless dry-run is off and the latest report is recent |
| Tests | `octo.Tests/Retention/` |
| Dependency | This section follows `docs/modules.md`, which comes from FLA-8 and is not on `main` yet |
| Hotspot edits | The one `Program.cs` line; the snapshot trigger needs a call from the Subsonic proxy path (it observes a verified request). That is an edit to `SubSonicController*.cs`, a hotspot, and per `docs/modules.md` should be a single call into the module after the credential check, claimed first. Alternative that avoids it: take the snapshot from the existing `CredentialCheck`/`RequestIdentity` path, which is not a hotspot. FLA-24 picks one |

Off means off: with the switch off the module makes no calls, writes no files, takes no snapshots,
and its endpoints answer "off". Dropping the module (delete the folder and the registration line)
leaves the existing trash and quarantine behaviour untouched, because the build only appends one
enum value and one admin-only restore path.

## 9. Deletion safety (for the Security sign-off)

How each requirement of `docs/security-requirements.md` section 5 is met, and what is new.

| Req | Met by |
|---|---|
| R5.1 No direct delete; trash with manifest and retention | Only `LibraryActionQuarantine.Move`. Retention contains no `File.Delete`/`Directory.Delete`. The only real deletion is the existing `Sweep` after the trash window (section 6) |
| R5.2 Path inside root with separator boundary; regular file, no link | `NavidromeSongPathResolver.IsInside` appends the separator (`:328-334`) and `LibraryActionQuarantine.Outside` re-checks, refuses the trash itself, and refuses a reparse point (`:158-168`). **Gap to close in the build (Security finding 1, high):** `Outside` checks only the final file. A directory further up (`/music/Artist -> /elsewhere`) can be a symlink and `IsInside` compares text, so the file would pass. Fix in `Outside` itself, so every action that uses `Move` benefits: resolve the real path of the music root first (the root itself may be a link), require the file's real path to be inside it with a separator boundary, refuse if any directory between the root and the file is a link, and require a regular file, not a directory. Apply it to the move source, the trash destination and `Restore`, and **repeat it immediately before each move** (conditions 1, 2) |
| (new) Links and devices | A file with a link count above 1 (shared with a seeding folder or the Soulseek share) is skipped and reported as protected, since only its library name would move and no space would be freed (condition 4). The trash must be on the same device as the file, and the copy-then-delete fallback in `Move` (`:117-126`, length check then `File.Delete`) is refused for retention (condition 5). Paths under `.octo-incoming` or the trash, and any file not in `.mappings.json`, are refused in code (condition 3) |
| R5.3 Dry run default, reports, hard cap per run | Section 7. `DryRun` true, report always written, caps abort the run, upper bounds hard-coded |
| R5.4 Decision on an id Octo issued, not on text | The candidate is the Navidrome id resolved by the native leg and equal to Octo's own mapping path (4.2). No user-supplied text becomes a path. Titles and artists are used for display only |
| R5.5 Cleanup deletes only inside one named folder Octo created | Not applicable to the move (it acts on the library on purpose, only on files Octo downloaded, section 3). The sweep deletes only dated folders in `.octo-trash`. Hardening: `Sweep` should skip a dated directory that is a reparse point |
| R5.6 No recursive delete on a configured or user-supplied path | `Sweep` does `Directory.Delete(dated, recursive: true)` on a dated child of the trash folder, a folder Octo created. `EffectiveQuarantineDirectory` is a single sanitised segment (`LibraryActionSettings.cs`). Retention adds no recursive delete |
| R5.7 Journal per deletion that survives restart | One journal entry per move, written and flushed **before** the file moves (the pattern at `LibraryActionExecutor.cs:236-246`); a crash leaves `Pending`, which startup reconciles (`Reconcile`). The entry records the retention reason in plain words ("not played by anyone since <date>, in no playlist"). The manifest beside the file lets a restore work without the journal |
| R5.8 Tests | See below |

**Further safeguards specific to this feature**

1. Fail closed: every unknown, error, partial read or uncovered user ends in "keep" (sections 2.4,
   4.1, 5). The evaluator is a pure function over complete input, so "I could not read X" cannot
   be expressed as "X is empty".
2. Two-step removal: dwell of 24 h (4.4), then move to the trash, then a 30-day trash window, and
   only then the existing sweep.
3. No user input chooses a path or an id to remove. The admin endpoints can start a dry run or
   approve the latest report's run; they cannot name a file.
4. Caps abort rather than truncate (section 7).
5. Admin only. The apply action needs a Navidrome admin session, not a recovery-code session
   (R3.3), plus `AdminRequestGuard` on the POST (R3.2).
6. No secret is stored. Snapshots contain song ids and dates per user, kept apart from
   `settings.json`, removed when the user is deleted in Navidrome (R2.2, R2.8). The per-user data
   is never shown to another non-admin user; the report is admin-only.
7. Idempotent and re-entrant: a lock prevents two runs at once; the journal's `Pending` entries
   stop a second run double-moving.

**Tests required in FLA-24** (R5.8, R2.7, and the failure cases)

- Paths: with `..`; absolute path outside the root; sibling with the same prefix (`/music2` next
  to `/music`); a symlinked file; a **symlinked parent directory**; a directory where a file is
  expected; a path inside the trash.
- The evaluator, one case per condition E1-E10 showing a single "no" keeps the song, and a table
  of "cannot tell" inputs (null, empty list from a failed call, unparsable date, future date) all
  keeping it.
- Failure injection against a fake Navidrome: unreachable; 500 on one page; timeout; playlist
  list truncated at the cap; playlist with more than 500 tracks; user list truncated; a user with
  no snapshot; a stale snapshot; an empty library; a scan in progress; the admin token refused.
  Every one ends in no move and a report that says why.
- Per-user: a play by user B only, a star by user C only, a playlist of user D only, each protects
  the song; the same song with all four removed becomes a candidate (and not before its dwell).
- The caps: 26 candidates with `MaxPerRun` 25 aborts and moves none.
- Dry run moves nothing and still writes the report; going live moves only candidates past their
  dwell.
- Journal and crash: the move is refused when the journal cannot be flushed; a `Pending` entry is
  reconciled after a restart.
- Download after removal: a retention-removed song can be hearted and downloads again (not
  `BlocksDownloads`).
- Restore puts the file back, re-links the mapping, and is admin-only.
- The Lidarr rules: busy claim, track file present, Lidarr unreachable.
- Module off: no outgoing call, no file written.

## 10. Needs a human decision

1. **Ratings (E5).** Any rating protects a song, including 1 star. That keeps songs a user disliked.
   A 1-star rating is also the "Delete" command when rating actions are on, handled by its own
   worker. Treat 1-2 stars as "not protected"? Recommended: no, keep it conservative for v1.
2. **Album and artist hearts (E4)** protecting every song of the album/artist. Conservative and
   will keep more disk. Recommended: keep as designed.
3. **Option A vs B vs C** (2.3). A needs no approval. Security does not recommend B (read-only
   database mount) or C (stored API keys); each needs written human approval (condition 19).
4. **Uncovered users pause cleanup** (2.4). Confirm that is acceptable on this server, and who
   maintains `IgnoredUsers`.
5. **Where the snapshot trigger lives** (section 8): in the proxy path (a `SubSonicController*.cs`
   hotspot edit) or in `CredentialCheck`/`RequestIdentity`.
6. **Pin Navidrome.** Compose uses `deluan/navidrome:latest`; several **verify** items depend on
   native API behaviour that can change with it.
7. Going live (`RETENTION_DRY_RUN=false`) on real data requires written human approval.

## 11. Conditions for the build (Security, FLA-16 sign-off)

Source: the Linear comment `[Security agent] Approved (design).` on FLA-16, which reviewed this
document at `fa22024` against `docs/security-requirements.md` section 5, R1 and R2. All 19 are
required of FLA-24, and Security checks them on that PR. Summarised; the comment is the record.
Where the body above changed because of one, the section is named.

| # | Condition | Body |
|---|---|---|
| 1 | Real-path confinement in `Outside`: resolve the real path of the root, require the file's real path inside it with a separator boundary, refuse a link in any directory between, require a regular file. Applied to move source, trash destination and `Restore`, and done immediately before each move | 9 |
| 2 | Tests: symlinked parent directory, symlinked trash root, symlinked dated trash folder, symlinked ancestor on restore, each ending in no move | 9 |
| 3 | Refuse `.octo-incoming` and the trash in code, and any file not in `.mappings.json` | 3, 9 |
| 4 | Skip and report as protected any file with a link count above 1 | 9 |
| 5 | Same-device trash only; no copy-then-delete fallback; retention code has no `File.Delete` or `Directory.Delete` | 9 |
| 6 | A song is moved only if every covered user's snapshot was taken after its first evaluation as a candidate; the move-time recheck of E1 and E6 stands | 4.4 |
| 7 | At move time recapture size, last write time and file id and compare; take the per-song busy lock shared with library actions and downloads; any change cancels | 4.4 |
| 8 | Clock sanity: skip (`skipped_clock`) if `T` is before the last run, more than about 3 days after it, or differs from Navidrome's time by more than a few minutes | 4.4, 5 |
| 9 | The sweep retention runs applies the same clock check, never runs in dry run or with the switch off, skips linked dated folders and does not follow links | 4.4, 6 |
| 10 | Rolling cap (for example 100 songs per 7 days) on top of 25 / 5 %, with hard upper bounds; abort on an empty or unreadable library count | 5, 7 |
| 11 | New `LibraryAction` value appended; `Resolve` handles it explicitly (Pending with file in trash: Applied; source present: Failed, no restore); tests for both; `BlocksDownloads` false, with a test that a hearted song downloads again | 6 |
| 12 | Report and trash manifest are the durable record: manifest written before the journal entry completes; reports kept for the full trash window | 6 |
| 13 | Snapshot and report files: temp file `0600` from the start (R1.4), apart from `settings.json`, in the config folder, one file per user for snapshots, written atomically; if the mode cannot be set the file is not used for per-user data; never served to a non-admin | 7 |
| 14 | Store the least that works (song id, last-played date, starred and rated flags); the report shows reason class, user names and only the latest play date per user; delete a snapshot when Navidrome no longer lists the user; delete reports after 30 days; README says the report shows listening dates to an Octo admin | 7 |
| 15 | Apply endpoint: POST only, under `/api/admin/retention`, Navidrome admin session (not recovery, R3.3) plus `AdminRequestGuard`; takes no path or song id; acts only on the latest stored report's candidates, re-verified at that time | 8 |
| 16 | Snapshot keyed by the username Navidrome returns for the verified credentials, never the `u` parameter alone; reads are all-or-nothing (a failed read keeps the old snapshot); credentials in memory only, never on disk or in logs, no query string logged; the trigger runs after the credential check; claim `SubSonicController*.cs` first if the proxy path is chosen | 2.3, 8 |
| 17 | Tests per section 9 plus the failure cases of conditions 1-12; with the module off, no outgoing call and no file written | 9 |
| 18 | Pin Navidrome to a version tag, or record its version in the report and abort when it changes from the one the verify items were checked against (`aborted_version`); settle every verify item in the FLA-12 harness before going live; a failed verify item makes the affected condition skip the run, never act | 2.2, 5, 10 |
| 19 | Going live (`RETENTION_DRY_RUN=false`) on real data, and choosing option B or C, each need written human approval; the first live run must follow a dry-run report a human has read | 7, 10 |

**QA gaps folded in** (QA comment on FLA-16): the post-snapshot window (condition 6); a runtime
fail-closed check that the admin sees every user's private playlists (section 2.2,
`skipped_playlist_visibility`); UTC-only dates, rejecting `played` values without an offset
(E3); and the `docs/modules.md` dependency on FLA-8 (header and section 8).
