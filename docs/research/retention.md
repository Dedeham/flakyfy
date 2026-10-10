# Design: 30-day retention cleanup

Issue FLA-16. Written for the Security sign-off, and as the input to the build issue FLA-24.
This is a design. It changes no code and nothing in it has been run. Every claim about Octo cites
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
  Reading private playlists of others as admin is therefore already how Octo works. **verify**
  that the native list returns private playlists of all users on the pinned version.
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
| B | Navidrome's database mounted read-only into Octo and read directly | All users, all history, no credentials | A new mount and a dependency on Navidrome's internal schema (**verify**), which can change between releases; compose currently uses `deluan/navidrome:latest` (`docker-compose.yml:474`) | Needs human approval. Offer as an opt-in alternative |
| C | Each user hands Octo a long-lived Navidrome API key | Everyone who does it | A new stored credential class. `docs/security-requirements.md` R1.5 allows tokens a user hands Octo if stored apart from `settings.json`, keyed by user, `0600`, and removed on disconnect, but a full-access key held at rest is the most sensitive thing Octo would keep | Not recommended. Needs human approval |
| D | A play ledger kept by Octo from the plays it proxies (`rest/scrobble`, `rest/stream`) | Plays through Octo only, from the day it starts | History is empty at first, so alone it can never show "no plays for 30 days" | Used only as a cheap extra check (section 4.4), not as the source |

**Decision for the design: Option A, with the extra checks from section 4.** B and C are listed so
the human can choose them; nothing below depends on them.

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
| E3 | **Last play, every user.** For every Navidrome user, the user's last played time for the song (`played`) is missing or at least `W` before `T`. A time that cannot be parsed, or is in the future, counts as recent | one snapshot per user (2.3 A); parse like `ForYouLists.Played` (`:170`) | "Missing" is only trusted for a covered user |
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
- **Snapshots are bounded in age** (14 days) and a play that happened after the snapshot is the
  gap that the 30-day trash (section 6) exists to cover, along with the ledger recheck.

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
| Library looks wrong: the music root is missing or empty, or Navidrome's song count dropped more than 20 % since the last run | `aborted_library_changed` |
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
   trash window is shared.
2. **A retention removal must not block re-download.** A Delete action's journal entry makes the
   song "not downloaded again" (`LibraryActionEntry.BlocksDownloads`, `LibraryActionJournal.cs:54`).
   A song removed for being unplayed is not something the user rejected: if somebody hearts it
   later it must download again. The retention entries use a new `LibraryAction` value, appended
   at the end of the enum (the journal stores numbers, `LibraryActionSettings.cs`, the comment on
   `LibraryAction`), and `BlocksDownloads` must be false for it. Restore from the dashboard's
   trash list (`LibraryEditService.RestoreAsync` :191) currently matches only `Delete` entries,
   so restoring a retention entry needs a small, admin-only addition there (R3.3).
3. **Navidrome only forgets the song at the next scan.** After a move, request a rescan the way
   the actions do (`LibraryRescan.Soon`, `LibraryEditService.cs:185-209`), and drop the mapping
   (`ForgetMappingAsync`, kept in the manifest so a restore re-links it).

Shared journal limit: the journal keeps 2000 entries (`LibraryActionJournal.cs:74`), so a run's
full report is kept in the module's own file (section 8), and the journal holds one entry per
moved song.

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
  Hard-coded upper bounds apply whatever is configured.
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
  Usernames and titles appear in the report, so the endpoint is admin-only and answers
  `Cache-Control: no-store` (R3.3, R3.6).

## 8. Module boundary (per `docs/modules.md`)

| Part | Content |
|---|---|
| Folder | `octo/Services/Retention/`: `RetentionModule.cs` (the extension method), `RetentionSettings.cs`, `RetentionWorker` (hosted service, runs once a day at a configured hour, checks the switch at run time), `RetentionEvaluator` (pure: the table in section 4), `NavidromeReader` (strict readers, 4.1), `UserSnapshotStore`, `RetentionReportStore`, `RetentionState` (dwell) |
| Switch | `RETENTION_ENABLED`, default `false`, in the four places: `.env.example`, `docker-compose.yml` (`Retention__Enabled: ${RETENTION_ENABLED:-false}`), `appsettings.json` `"Retention": { "Enabled": false }`, `RetentionSettings.Enabled = false` |
| Other settings (same four places) | `RETENTION_DRY_RUN` true, `RETENTION_DAYS` 30, `RETENTION_MAX_PER_RUN` 25, `RETENTION_MAX_LIBRARY_PERCENT` 5, `RETENTION_SNAPSHOT_MAX_AGE_DAYS` 14, `RETENTION_IGNORED_USERS` empty, `RETENTION_RUN_HOUR` 4 |
| Registration | one line in `Program.cs`: `builder.Services.AddRetentionModule(builder.Configuration);` |
| Controller | `octo/Controllers/RetentionController.cs`, under `/api/admin/retention`: read the latest report, run a dry-run now, apply. All admin-session only; the apply action refuses unless dry-run is off and the latest report is recent |
| Tests | `octo.Tests/Retention/` |
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
| R5.2 Path inside root with separator boundary; regular file, no link | `NavidromeSongPathResolver.IsInside` appends the separator (`:328-334`) and `LibraryActionQuarantine.Outside` re-checks, refuses the trash itself, and refuses a reparse point (`:158-168`). **Gap to close in the build:** `Outside` checks only the final file. A directory further up (`/music/Artist -> /elsewhere`) can be a symlink and `IsInside` compares text, so the file would pass. Retention must resolve the real path of the file and of the music root (`ResolveLinkTarget` on every ancestor, or a realpath) and require the real path to be inside the real root, refuse if any ancestor is a link, and require `FileInfo.Exists` as a regular file (not a directory). Add the same to `Outside` since the other actions benefit |
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
3. **Option A vs B vs C** (2.3). A needs no approval. B (read-only database mount) and C (stored API
   keys) each need written approval under `AGENTS.md` ("adding a new ... dependency").
4. **Uncovered users pause cleanup** (2.4). Confirm that is acceptable on this server, and who
   maintains `IgnoredUsers`.
5. **Where the snapshot trigger lives** (section 8): in the proxy path (a `SubSonicController*.cs`
   hotspot edit) or in `CredentialCheck`/`RequestIdentity`.
6. **Pin Navidrome.** Compose uses `deluan/navidrome:latest`; several **verify** items depend on
   native API behaviour that can change with it.
7. Going live (`RETENTION_DRY_RUN=false`) on real data requires written human approval.
