# Rust port backlog

<!-- markdownlint-configure-file
{ "MD013": false }
-->

The Rust server in `crates/` is at parity with C# 2026.10.04. The C# app in
`octo/` is at 2026.10.05 and moving. This file is the working list for closing
that gap: every C# change since the Rust baseline, grouped, with its parity
diffs and its status. Tick a commit when its behavior, its tests (a
`docs/rust-migration/test-map.md` row each) and its parity cases are on
`rust/main`.

Baseline: Rust at 2026.10.04 parity. Target: `main` at `ce63143` (release
2026.10.05 plus a README commit). The gap is `git log --no-merges
89a4be1..ce63143`, 121 commits, all listed below.

## Working on the port

- **Where cargo runs.** WSL Ubuntu, on its own filesystem (not `/mnt/c`).
  Windows can `cargo check --workspace --tests` for quick edits (2 min 8 s
  cold), but the test suite is Linux only.
- **One worktree, one `target/`.** Each worktree builds into its own
  `target/`; never point several at one `CARGO_TARGET_DIR`. Incremental
  builds are on, so a `target/` grows to about 7.3 GB (3.8 GB of it
  incremental caches). Keep 30 GB free in WSL.
- **One memory budget in WSL.** Run cargo through `~/bin/cargo-slot`. It is
  now a weighted budget of WSL's 16 GB for builds and parity stacks together
  (`parity/budget.sh`, README "WSL's budget"): 12.5 GB, a cargo build 3.5 GB
  (a test rebuild peaks at 2.3 to 3.4 GB with incremental caches, 4 to
  5.3 GB cold), a parity stack 1.25 GB. A build that does not fit waits in
  line; a stack never takes the last build's share. `cargo-slot --status`
  shows who holds what.
- **Parity runs: `parity/run.sh`.** One command, the options of
  `parity/lxc131.sh`. It runs in WSL's own Docker by default, as many stacks
  at once as the budget allows (the runs of several branches side by side),
  or on the test box with `--lane box`. The run outlives the script:
  `--attach` follows it again and fetches its reports, `--queue` shows both
  lanes. `parity/lxc131.sh` still works for the box.
- **The loop.** `cargo test -p <crate> <filter>` while working; the full
  `cargo test --workspace`, `cargo clippy --workspace --tests -- -D warnings`
  and `cargo fmt --all --check` before merging into `rust/main`.
- **Shared files.** The Rust build reads the C# app's own files: the
  dashboard (`octo/wwwroot`), `octo/Assets`, the cover design
  (`octo/Services/CoverArt/Design`), `octo/appsettings.json`, the golden
  cover samples (`octo.Tests/CoverGolden`), the shared case files
  (`docs/*-cases.json`) and the release number (`octo/octo.csproj`). A debug
  run serves the dashboard from `octo/wwwroot` with no env vars.
- **Port behavior, then improve on purpose.** Follow
  `docs/rust-migration/CONVENTIONS.md`. A deliberate improvement over the
  C# behavior is fine: give it a `docs/rust-migration/known-diffs.md` row
  (C#, Rust, why) and a parity allowlist entry pointing at it.

## Phase 0 record (2026-10-06)

| Item | Result |
| --- | --- |
| Workspace | Imported at 2026.10.04 parity; `sonic/` excluded (cargo refuses to run in `sonic/` without it; checked) |
| `cargo build --release` (WSL, 16 cores, cold, crates already downloaded) | 2 min 40 s wall, 16 min CPU; peak memory 3.0 GB over idle (the LTO link alone 3.1 GB); binary 33.7 MB |
| `cargo test --workspace` cold | 6 min 18 s wall; peak 4.7 GB over idle |
| Tests | 2,470 pass, 0 fail (octo 1,574 + 3, octo-core 648, octo-media 132, octo-subsonic 113) |
| The 7 failures seen in planning | Environment only: under WSL mirrored networking a closed port such as 127.0.0.1:1 times out instead of refusing. Fixed: the tests use a port bound and let go, which refuses everywhere; the `octo` tests went from 187 s to 51 s |
| Shared song identity cases | 26 of the 2026.10.05 cases fail in Rust (group B); skipped by name in `song_identity_tests.rs`, and a skipped case that starts passing fails the test until it is taken off the list |
| `cargo clippy --workspace --tests -- -D warnings` | Clean, 2 min 41 s cold |
| Edit and rebuild the tests, incremental off (the fork's setting) | Edit in `octo` 89 s, peak 4.0 GB; edit in `octo-core` 87 s, peak 3.9 GB |
| Same, incremental on (now the default) | Edit in `octo` 12 s, peak 2.3 GB; edit in `octo-core` 28 s, peak 3.4 GB |
| `target/` size | 2.8 GB without incremental caches, 7.3 GB with them |
| Windows | `cargo check --workspace --tests` passes (the symlink test is Unix only) |
| `Dockerfile.rust` on LXC 131 (4 cores, cold, own buildx builder capped at 3.5 GB) | 5 min 12 s in all: dependencies (cargo-chef cook) 1 min 44 s, the Octo crates 2 min 43 s; image 993 MB |
| Workflows | `rust.yml`, `docker-rust.yml`, `ci.yml` pass actionlint 1.7.12 with shellcheck, apart from `concurrency.queue`, which actionlint does not know yet (`docker.yml` has the same line and runs) |

Not measured yet, because nothing is pushed: the CI run times on GitHub's
runners, the GitHub Actions cache sizes for the `octo-rust-amd64` and
`octo-rust-arm64` scopes against the repository's 10 GB, and the `sonic` job
on `rust/main`. Record them after the first push of `rust/main` and the first
manual run of the Rust image workflow.

## Parity: first run against C# 2026.10.05

Run on LXC 131 on 2026-10-06 with the corpus as it came from the Rust port
(453 requests, 12 files), the published image
`ghcr.io/winters27/octo:2026.10.05` against `Dockerfile.rust` built from
`rust/main` (`19b375f`).

| Comparison | Structural | Bytes |
| --- | --- | --- |
| C# against C# (two runs) | 453 identical | 453 identical |
| Rust against Rust (two runs) | 453 identical | 453 identical |
| C# against Rust | 172 unexplained, 281 explained by the allowlist | 172 unexplained |

So the corpus as it stands is deterministic on both builds, and every diff is
a real difference.

Diffs by corpus file:

| Corpus file | Requests | Diffs |
| --- | ---: | ---: |
| `00-setup` | 6 | 1 |
| `01-system` | 43 | 4 |
| `02-browsing` | 65 | 5 |
| `03-search` | 25 | 5 |
| `04-media` | 42 | 2 |
| `05-playlists` | 19 | 3 |
| `06-lyrics` | 24 | 0 |
| `07-octo-extensions` | 15 | 2 |
| `08-catchall-native` | 34 | 4 |
| `09-admin` | 97 | 97 |
| `10-static-cors` | 25 | 18 |
| `90-mutations` | 58 | 31 |
| **Total** | **453** | **172** |

Diffs by gap group (one request can show several groups):

| Group | Diffs | What differs |
| --- | ---: | --- |
| J | 148 | The sign-in gate (401 with `signIn` where Rust answers), the security headers on admin answers (`Content-Security-Policy`, `X-Frame-Options`, `Referrer-Policy`, `X-Content-Type-Options`, `Cache-Control: no-store`), the `browse/session` fields `recovery`, `signInOff`, `user`; 144 of the 172 are J alone |
| P | 10 | `explicitStatus` on outside songs and albums |
| Q | 8 | The Made for you playlist in `getPlaylists`; `octoLists` extension |
| G | 5 | `octoTopSongs` extension; `getTopSongs` answered by Octo (no relayed `Vary: Origin`) |
| D | 4 | `octoAcquisitions` extension at versions 1 and 2 |
| H | 4 | `octoImports` extension |
| N | 3 | `pending` on `genre/backfill` (the activity view) |
| B | 2 | `Content-Disposition` on a relayed download |
| F | 2 | `getLibraryActions` lists 6 actions; `libraryAction` knows retag, joinAlbum, cover, lookup, undo, restore |
| O | 2 | `ownedCount` on outside albums |
| K | 1 | `GET /` is a 302 to `app/` |
| L | 1 | `services.sonic` in `/api/admin/status` |

Read with care: the C# gate answers 401 to almost every admin request, so the
admin features of other groups (D, E, F, G, H, L, N, P, Q, R) cannot show yet.
Phase 1 signs the corpus in and adds the corpus groups below; most of the
backlog only becomes visible then. Groups A, C, E, I, M, R, S, T and U have no
diff in this corpus.

Repeat it (on a box with Docker; LXC 131 used its own folder and compose
projects, cleaned up after):

```bash
docker build -f Dockerfile.rust -t octo-rust:dev .
python3 parity/parity.py run --project p0cs --out /tmp/parity/cs
python3 parity/parity.py run --image octo-rust:dev --project p0rs \
    --port 18580 --nd-port 18553 --out /tmp/parity/rs
python3 parity/parity.py diff /tmp/parity/cs /tmp/parity/rs
```

## Phase 1: the harness at 2026.10.05

Done 2026-10-07 (`parity/README.md` has the details):

- [x] The corpus signs in for real (`00-setup` makes an admin and a recovery
  code sign-in; `"session"` on a request sends the cookie), and keeps the
  signed-out answers: `11-signin` asks every `/api/admin` GET signed out and
  signed in, `99-signed-out-writes` every write signed out (last, the restart
  last of all). Both are written from the C# controllers by
  `parity/tools/admin_routes.py`, whose `check` fails when a route is missing.
- [x] `ADMIN_SIGN_IN=off` and `ADMIN_PORT`: a second Octo in the parity stack
  (`octo-alt`, `"server": "alt"` and `"alt-admin"`).
- [x] Corpus groups for every 2026.10.05 group: `12-imports`, `13-downloads`,
  `14-settings-media`, `15-radio`, `16-lists`, `17-covers-explicit`,
  `18-dashboard`, `19-health`, `20-library-edits`, `21-static`,
  `95-acquisition` (two hearts and a pick end to end through the slskd stub,
  with a real fixture FLAC and M4A), `96-covers-explicit-writes`. 1,836
  requests in 25 files (453 before).
- [x] Stubs: Spotify accounts, API and embed pages, ListenBrainz labs, the
  shim's YouTube Music answers, octo-sonic, Deezer and Last.fm charts, slskd
  search results, transfers (the stub writes the file into Octo's job folder),
  options, YAML, shares and uploads, the Soulseek port test. New stub features:
  `times`, `bodyRegex`, `searchText`, `writeFile`, `{{size:...}}`, dated
  templates.
- [x] Determinism rule: timers off or waited for, random values captured,
  C#'s own unordered answers compared unordered; what cannot be made
  repeatable is listed in `parity/README.md` ("Determinism rule"). No
  test-only seed env var: the C# image is the published one, so only its own
  settings can hold it still.
- [x] Gate: C# against C# 0 diffs and Rust against Rust 0 diffs on the whole
  corpus, structural and bytes (record below).
- [x] One script for the test box: `parity/lxc131.sh` (README, "For port
  agents").
- [x] Parallel parity lanes in WSL: `parity/run.sh` picks WSL or the box, and
  WSL's `cargo-slot` is one memory budget for builds and stacks
  (`parity/budget.sh`; README "The WSL lane", record below).
- [ ] `parity/recordings/csharp` stays the 2026.10.03.2 baseline: the Rust
  golden tests (`subsonic_wire_golden_tests.rs`) pin the wire format the port
  has today. A group that changes those answers moves its golden tests onto a
  fresh 2026.10.05 recording in its own commit (`parity.py run`, then copy the
  corpus files it needs).
- [x] Regenerate the state fixtures with the C# generator at 2026.10.05
  (`docs/rust-migration/fixtures/state`): done in wave G1, with the 7 new
  files, `admin-recovery-code` and the new fields (state-files.md §4.26 on).
- [ ] Regenerate the tag fixtures (`fixtures/tags`) at 2026.10.05: group C
  (the explicit advisory, `ITUNESADVISORY` and `rtng`), with its tagging.

## Phase 1 record (2026-10-07)

Run on LXC 131 from `rust/main` with `parity/lxc131.sh --csharp-twice
--rust-twice --bytes`: the published `ghcr.io/winters27/octo:2026.10.05`
against the Rust build of this branch (still at 2026.10.04 behavior).

| Comparison | Structural | Bytes |
| --- | --- | --- |
| C# against C# | 1,836 identical | 1,836 identical |
| Rust against Rust | 1,836 identical | 1,836 identical |
| C# against Rust | 1,458 unexplained, 360 explained by the allowlist, 18 identical | the same |

A C# recording of the whole corpus takes about 25 minutes, a Rust one about
30 (the waits for features it lacks time out; `--until-patience` keeps that
bounded). The two gate runs were also made on the corpus before its last
compare-only fixes (ignored keys, member order), then compared again with the
final corpus: 0 in all four comparisons.

**The port's worklist.** Requests with an unexplained diff, per backlog group.
"Own": requests tagged with the group that differ. "Diffs": requests anywhere
whose differing part `parity/groups.json` puts on the group (J counts the
missing admin security headers on every admin answer; L, P, Q count settings
and extension lines in other groups' answers). One request can count in
several groups. Single: requests whose only differing group is this one, so
porting the group alone makes them pass.

| Group | Tagged | Own | Diffs | Single |
| --- | ---: | ---: | ---: | ---: |
| A Last.fm search cleanup | 5 | 5 | 5 | 5 |
| B Download matching | 19 | 18 | 22 | 9 |
| C Tagging polish | 16 | 15 | 15 | 3 |
| D Download log, Find songs | 144 | 133 | 131 | 64 |
| E slskd sharing | 45 | 45 | 45 | 1 |
| F Library edits, trash | 157 | 140 | 145 | 110 |
| G Top songs, charts | 57 | 55 | 56 | 37 |
| H Spotify import | 208 | 208 | 207 | 96 |
| I Secret masking | 78 | 76 | 76 | 2 |
| J Admin sign-in | 262 | 252 | 927 | 225 |
| K Bare address 302 | 6 | 5 | 6 | 5 |
| L Multi-source radio | 87 | 75 | 136 | 29 |
| N Activity, Afterglow | 48 | 48 | 44 | 6 |
| O Album ownership | 21 | 20 | 26 | 1 |
| P Covers, explicit | 114 | 106 | 145 | 38 |
| Q Made for you | 20 | 14 | 63 | 6 |
| R Library health | 150 | 137 | 130 | 45 |
| S Stream length | 25 | 5 | 4 | 5 |
| base (2026.10.04 behavior) | 479 | 204 | 164 | 18 |

The 18 single "base" diffs are knock-on effects of the groups, not
regressions: the OpenSubsonic extensions list (2026.10.05 lists 12, Rust
9), the Made for you playlists in `getPlaylists`, the relayed download file
name, and `90-mutations`
reading a library that `20-library-edits` changed. J goes first (wave G1): 225
requests differ only by the gate and the admin security headers.

Diffs per corpus file: 00-setup 4, 01-system 4, 02-browsing 5, 03-search 5,
04-media 2, 05-playlists 3, 07-octo-extensions 2, 08-catchall-native 4,
09-admin 97, 10-static-cors 18, 11-signin 130, 12-imports 206, 13-downloads
108, 14-settings-media 149, 15-radio 74, 16-lists 60, 17-covers-explicit 74,
18-dashboard 63, 19-health 101, 20-library-edits 136, 21-static 31,
90-mutations 36, 95-acquisition 35, 96-covers-explicit-writes 34,
99-signed-out-writes 77. 06-lyrics has none.

**Intentional diffs.** `parity/intentional-diffs.json` holds the deliberate
improvements, one reviewed entry per change (requests named, the diff lines
pinned by regex, a reason, and `reviewed` by Brandon for anything a user or an
app sees). Wave G1 proposed the first three (`reviewed` unset, so they explain nothing
until Brandon signs them): `recovery-refused-on-library-files`,
`lidarr-test-saved-key-stays-home` and `config-sources-fixed-mask`, the port
fixes below that the corpus reaches. The list under "Improvements we want while
porting" below is where the rest come from.

### Found while building the harness

C# 2026.10.05 behavior the corpus records as it is. **Decided 2026-10-07
(Brandon): all eleven are fixed in the Rust port only, as intentional diffs**
(a known-diffs.md row and a `parity/intentional-diffs.json` entry each,
`reviewed` left for Brandon before shipping). Five are fixed in wave G1
(marked below); the other six carry a "fix in port" line under their group.

- **"Keep as playlist" on an import answers 500** against Navidrome 0.64.2:
  Octo reads Navidrome's answer to the playlist create as JSON, and it is not
  (`NavidromePlaylistApi.cs:143`; likely the `application/json; charset=utf-8`
  content type, so Navidrome takes the body as an M3U import). Navidrome makes
  an empty playlist named after the time, the list stays marked as kept, and
  the 30 s import loop makes another one on every retry. `12-imports` turns
  each playlist off at once and deletes what it leaves.
- **Clearing an ISRC does nothing on disk** (`TagWriterExtras.cs:98-101` returns
  on an empty value), so undoing an ISRC retag answers `isrc: null` and leaves
  the tag in the file.
- **The explicit backfill accepts a recovery code session**
  (`ExplicitBackfillController.cs:30`, `UserOf`), so the recovery code can
  write explicit marks into music files; the cover upgrade refuses it.
  **Fixed in G1** (`4d38b38`): the gate refuses a recovery-code session on
  every route that reads or changes library files
  (`admin_sign_in_gate::LIBRARY_FILE_ROUTES`: browse, tags/preview,
  library-actions, notices, lossy, upgrades, library/resolve, genre/backfill,
  covers, lyrics, imports, downloads/log, find, trash, health, explicit),
  with the words each endpoint answered; group P's controller needs no check
  of its own.
- **`lidarr/test` sends the saved Lidarr key to any address typed in** when the
  placeholder is sent (`AdminController.cs:2372`); the "a secret never follows
  a new address" rule covers saves only.
  **Fixed in G1** (`4d38b38`, `lidarr_test_key`): the saved key follows the
  saved address only; another address answers 400 "Retype the Lidarr API key
  for the new address.". The other test routes send nothing saved to a typed
  address: `lastfm/check` (fixed Last.fm host), `listenbrainz/validate`
  (fixed ListenBrainz host), `test-notification` (the saved ntfy and
  Discord addresses only), `soulseek/test-port` (no secret), the Spotify
  connect (a client id, not a secret).
- **A Navidrome address saved with a trailing slash** passes the same-address
  rule, and the stream relay then calls `//rest/stream`
  (`SubsonicProxyService.cs:344`). **Fixed in G1** (`4d38b38`): saved
  trimmed and without trailing slashes (settings and Raw config), and read
  that way (`normalize_server_url` when the settings bind).
- **Section names in a settings save:** the secret checks ignore case, the file
  writer does not, so `"lidarr"` writes a second section the config loader
  rejects on reload. **Fixed in G1** (`4d38b38`,
  `writer::canonical_sections`): a known section is written under its own
  name, and two spellings of one become one.
- **`config-sources` shows a secret's length** (up to 16 bullets). **Fixed in
  G1** (`4d38b38`): eight bullets for every set secret.

The two G1 fixes no corpus request reaches (Navidrome's address with a trailing
slash, a section name in another case) have their known-diffs.md rows and Rust
tests but no intentional-diffs.json entry: a parity entry names the requests
whose answers differ, and none do. A corpus request for each would need its own
C# recording run; a settings save with `http://navidrome:4533/` would also move
every later request in its corpus file onto the normalized address.
- **Resuming a cancelled cover run counts `kept` and `albumsDone` again**
  ("6 of 3 albums", `CoverUpgrade.cs:696,744`).
- **The cover scan walks into `/music/.octo-trash`**, and the album id of a
  trashed song depends on the dated folder.
- **An outside song's registry id ignores its album** (`ExternalIdRegistry.cs`,
  `MakeShortId`): radio registers the Zephyr Echo songs from Last.fm without an
  album and replaces the search's entry, so later answers lose the album until
  a search registers them again (`95-acquisition` starts with that search).
- **The Soulseek web address is saved with the spaces it was typed with.**
  **Fixed in G2 with E** (`port/g2-eg`): saved (settings and Raw config,
  `normalize_saved_addresses`) and read (`AppSettings::bind`) trimmed; the proposed
  intentional diff is `slskd-web-url-trimmed` (`18-dashboard/settings-weburl-ok`, `-ok-read`).

### G1 record: groups A, K and S (2026-10-07)

Run on LXC 131 from `port/g1-aks` (`7411096`) with `parity/lxc131.sh --no-build --rust-twice
--bytes`, against the cached C# recording of the same corpus.

| Comparison | Structural | Bytes |
| --- | --- | --- |
| C# against Rust | 1,433 unexplained (1,458 in phase 1), 385 explained, 18 identical | the same |
| Rust against Rust | 1,835 identical, 1 unexplained | the same |

| Group | Own before | Own after | Diffs before | Diffs after |
| --- | ---: | ---: | ---: | ---: |
| A | 5 | 0 | 5 | 0 |
| K | 5 | 0 | 6 | 0 |
| S | 5 | 0 | 4 | 0 |
| B (`0a700c0`, the relayed file name) | 18 | 11 | 22 | 11 |
| base | 204 | 201 | 164 | 163 |

Every other group is as in phase 1 except R, one more (138 own, 131 diffs): the Rust against
Rust diff, `19-health/plant-wait`. That request stops polling once the 7 planted songs are
listed, while Navidrome's full scan is still filling in album covers and MusicBrainz ids, so
which album comes first and whether it has a cover depends on the scan's timing. It touches
none of A, K and S, and it was identical in the phase 1 gate runs; the fix is a wait for the
scan to finish (`getScanStatus`) before it, in the corpus.

The image was not built by the harness: the Proxmox host had about 3.6 GB to spare (another
container's load swings by 4 GB a minute), and both the usual build and `--low-memory` tripped
the watchdog while compiling the `octo` crate. It was the same commit built in WSL as a static
musl binary (`cargo build --release --target x86_64-unknown-linux-musl`, since a WSL glibc
build needs glibc 2.38 and the image is bookworm's 2.36), put into `Dockerfile.rust`'s own
runtime stage on the box and tagged `octo-rust-parity:port-g1-aks`; the runs then used
`--no-build`. The targeted run first (`--only '[01][348]-*'`, 456 requests): A, K and S at 0;
03-search 5 (unchanged), 04-media 2 to 0, 08-catchall-native 4 to 3, 13-downloads 108 to 101,
14-settings-media 149 to 134, 18-dashboard 63 (unchanged).

### G1 record: groups J and I (2026-10-07)

Run on LXC 131 from `port/g1-core` (`25b28bc`, rebased on `rust/main` `4a17b3b`
with A, K and S) with `parity/lxc131.sh --bytes --rust-twice`: the binary
compiled in WSL, the corpus with `19-health/plant-scan-done` (so the C# side was
recorded again, 1,837 requests). The numbers below are that recording diffed again
with `978acef`'s allowlist (the two static-file entries also cover `21-static`
and the admin port's index) and the three proposed intentional diffs.

| Comparison | Structural | Bytes |
| --- | --- | --- |
| C# against Rust | 809 unexplained (1,433 after A, K and S), 1,004 explained, 24 identical | the same |
| Rust against Rust | 1,837 identical, 0 unexplained | the same |

| Group | Own before | Own after | Diffs before | Diffs after |
| --- | ---: | ---: | ---: | ---: |
| J | 252 | 6 | 927 | 45 |
| I | 76 | 3 | 76 | 3 |

What is left in J and I is the port fixes and one J bug the run found:

- I 3 and J 3 are the proposed intentional diffs: `config-sources` with eight
  bullets (6 requests) and `lidarr/test` refusing the saved key at a new address
  (1). With them reviewed, I is 0 and J is 3.
- J 3: a sign-out answered the gate's cookie renewal ahead of the cookie's
  deletion (`14-settings-media/sign-out-this-browser`, `sign-out-everywhere`,
  `recovery-sign-out-everywhere`; `90-mutations/browse-signout` the same).
  ASP.NET's `Cookies.Delete` took back the gate's `Append`, so the C# sent the
  deletion alone. Fixed in `05bf281` with a Rust test; the targeted run below
  checks it.
- J's 45 "diffs" are requests of other groups whose differing part
  `groups.json` puts on J too; none is J's own.

No group got worse. Own counts after, against the A, K, S record: B 11 (11),
C 12 (15), D 110 (133), E 21 (45), F 128 (140), G 48 (55), H 202 (208), L 45
(75), N 14 (48), O 20 (20), P 68 (106), Q 12 (14), R 97 (138), base 35 (201);
A, K and S 0. Most of the drop is the gate and the security headers, which every
admin answer carried. The other groups' recovery-code requests now meet the
gate's refusal (`17-covers-explicit/explicit-*-recovery*`, intentional) or
their unported route (`13-downloads/find-get-recovery`, D;
`18-dashboard/ambient-recovery`, `activity-recovery-signed`, N;
`18-dashboard/sharing-get-recovery`, E).

Rust against Rust is 0 again: `19-health` now waits for Navidrome's scan to
finish (`plant-scan-done`, `getScanStatus` until `scanning` is false) before it
lists the planted songs.

The harness now compiles in WSL by default (`parity/lxc131.sh`, `box-run.sh
--prebuilt`, README "Compile in WSL"); `--box-build` is the old way.

### G2 record: groups C, O and Q (2026-10-07)

Run on LXC 131 from `port/g2-coq` (`be07a5f`, merged with `rust/main` `13c48b5`) with
`parity/lxc131.sh --rust-twice`: the binary compiled in WSL, against the cached C# recording of
the corpus (1,837 requests). The box run outlived the local script, which stopped at its two
hour limit, and `box-run.sh` stopped with it before the C# against Rust diff; that diff was
then run on the box by hand with the same arguments.

| Comparison | Structural |
| --- | --- |
| C# against Rust | 786 unexplained (809 after J and I), 1,026 explained, 25 identical |
| Rust against Rust | 1,837 identical, 0 unexplained |

| Group | Own before (phase 1) | Own after J and I | Own after | Diffs before (phase 1) | Diffs after |
| --- | ---: | ---: | ---: | ---: | ---: |
| C | 15 | 12 | 9 | 15 | 9 |
| O | 20 | 20 | 17 | 26 | 0 |
| Q | 14 | 12 | 5 | 63 | 12 |

What is left is other groups' work, none of it C's, O's or Q's own:

- C 9: all `95-acquisition` (`heart-neon-*`, `pick-lima-*`). They wait on D's
  `getAcquisition` (404 in Rust), D's Find songs pick and the download log's `Key` and `Log`,
  and two `getSong` requests miss a capture those requests make. The tagging itself matches:
  the five `17-covers-explicit/tags-preview-*` requests (C and J) are identical now that J's
  headers are in.
- O 17 own, 0 diffs: every differing part of those requests is put on another group.
- Q 5 own, 12 diffs: `16-lists/listener-popular-*` (4) and the first list in
  `listener-playlists-lists-*` are G's Popular right now; `16-lists/extensions-*` (4) differ by
  G's `octoTopSongs`, H's `octoImports` and a second version of an earlier extension; the two
  `12-imports/nd-find-playlist-*` miss H's imported playlist, while the Deep Cuts list Octo
  makes beside it matches. `groups.json` puts Popular right now on Q as well as G.

No group got worse against the J and I record: B 11, D 110, E 21, F 128, G 47 (48), H 201
(202), I 3, J 3 (6), L 45, N 14, P 68, R 97, base 29 (35); A, K and S 0. R is tagged on 151
requests (150) with the corpus's `plant-scan-done`. The proposed intentional diff
`isrc-clear-takes-the-tag-off` matched its one aspect (`20-library-edits/tags-copy-undone`, an
F request); with it reviewed that request no longer differs for the ISRC.

Made for you list order was left out of the gate, as planned; Rust against Rust is 0 on the
whole corpus all the same.

### G2 record: group E (2026-10-08)

Branch `port/g2-eg`, rebased on `rust/main` `bbc1b1e`. Group G is not in it: it is owned by
`port/g-charts` (the Apple Music charts plan), and no G code was written here.

Run on LXC 131 from `port/g2-eg` (`662b0ba`, on `13c48b5`) with `parity/lxc131.sh --only
'1[18]-*' --bytes`: the binary compiled in WSL, the C# side recorded again for the subset (204
requests, 195 compared).

| Group | Own before (phase 1) | Own after J and I | Own after |
| --- | ---: | ---: | ---: |
| E | 45 | 21 | 5 |

What was left in E, structural and bytes alike:

- `18-dashboard/sharing-share-bad-body`: the C# answered `{"on": "maybe"}` with STJ's own
  words naming the record (`... converted to
  Octo.Controllers.SoulseekSharingController+ShareRequest. Path: $.on | LineNumber: 0 |
  BytePositionInLine: 14.`, under `$.on`, after `request`). Fixed after the run (`2537f75`,
  the switch's body refuses a member that is not a bool in those words; read from the box's C#
  recording, with a Rust test); the final run below confirms it.
- `18-dashboard/settings-weburl-ok` and `-ok-read`: the port fix, proposed as
  `slskd-web-url-trimmed` (`reviewed` unset, so still counted).
- `18-dashboard/activity-rescan-starting` and `-done` (tagged N and E): `/api/admin/activity`
  is N's route (404 in Rust); E's half, `SoulseekSharing::last_rescan_utc`, is in place for it.

So E's own count is 4 after the fix, 2 with the intentional diff reviewed, and 0 once N's
activity view lands. The other groups' own counts in the subset (D 3, F 3, H 2, I 0, J 3 for the
proposed `config-sources` bullets, L 1, N 14, P 1, R 3, base 3) are their own unported routes
and proposals; the subset cannot show the whole corpus, which the final run below was for.

The final run: `parity/lxc131.sh --rust-twice --bytes` on the whole corpus from `7d56cfe`
(rebased on `3c4c593`, the detached box run), against the cached C# recording (1,837 requests).
A first attempt on `fb3f28f` stopped before it built: the box had 7.0 GB free and needs 8 (this
branch's own image and folder were removed with `--clean`; the space came back once other runs
ended).

| Comparison | Structural | Bytes |
| --- | --- | --- |
| C# against Rust | 768 unexplained (786 after C, O and Q), 1,044 explained, 25 identical | the same |
| Rust against Rust | 1,837 identical, 0 unexplained | the same |

E is 4 own, as expected: `sharing-share-bad-body` now matches the C# byte for byte, and what is
left is `settings-weburl-ok` and `-ok-read` (the proposed `slskd-web-url-trimmed`, which matched
its 2 aspects in both modes and is waiting for review) and the two `activity-rescan-*` requests
of N. No group got worse against the C, O and Q record: B 11, C 9, D 110, F 128, G 47, H 201,
I 3, J 3, L 45, N 14, O 17, P 68, Q 5, R 96 (97), base 29; A, K and S 0.

### G3 record: group L (2026-10-08)

Branch `port/g3-l` (`1b4d021`, merged with `rust/main` `cd89293`). A first targeted run on
LXC 131 (`--only '1[15]-*'`, `694e9ee`, 218 requests compared) left nothing of L's own: every
`15-radio` diff was another group's. The final run: `parity/run.sh --rust-twice` on the WSL lane,
the whole corpus (1,837 requests) against the cached C# recording, 49 minutes.

| Comparison | Structural | Bytes |
| --- | --- | --- |
| C# against Rust | 741 unexplained (768 after E), 1,069 explained, 27 identical | not run |
| Rust against Rust | 1,837 identical, 0 unexplained | 1,837 identical, 0 unexplained |

| Group | Own before (phase 1) | Own after J and I | Own after | Diffs after |
| --- | ---: | ---: | ---: | ---: |
| L | 75 | 45 | 18 | 6 |

Song radio, station provenance, bans, learning, the outcome stats, Sounds alike's pass and
controls and the octo-sonic status card all match. What is left on L is other groups' work:

- L 18 own: 15 `getSimilarSongs` answers differ only by P's `explicitStatus` on one outside song;
  the 3 `extensions-radio-*` answers by D's second `octoAcquisitions` version and G's and H's
  extensions (octoRadioSources is listed and placed as the C# places it).
- L 6 diffs: those 3 extension answers, and `09-admin/update`, `11-signin/in-get-update` and
  `90-mutations/update-check`, whose update commands pull `yt-dlp-shim` and `octo-sonic`
  (`610dba2`, group T, the release phase); `groups.json` puts them on L for `octo-sonic`.

The registry fix shows in no request of this corpus: the C# answers carry the album too
(`95-acquisition` searches again before it reads the songs, as the harness notes said), so no
`intentional-diffs.json` entry is proposed: like G1's two unreachable fixes, it has a Rust test
and a known-diffs row. No group got worse against the E record: B 11, C 9, D 110, E 4, F 128,
G 47, H 201, I 3, J 3, N 14, O 17, P 68, Q 5, R 96, base 29; A, K and S 0.

The full run was started on the box first and stopped while queued; WSL full runs on `78234a5`
stopped silently after the image build (`run.sh`'s ADAPTIVE substitution under `set -e`), fixed
on `rust/main` in `cd89293`.

### WSL lane record (2026-10-08)

The parity harness now also runs in WSL's own Docker (29.1.3, compose 2.40), many stacks at
once, through `parity/run.sh` (README "The WSL lane"), under one memory budget with the cargo
builds (`parity/budget.sh`, installed as `~/bin/cargo-slot`: 12.5 GB, a build 3.5 GB, a C#
stack 1.25 GB, a stack of the Rust build 0.75 GB).

`rust/main` (`3c4c593`, the same code and corpus as the G2 record's `be07a5f`) with
`parity/run.sh --lane wsl --rust-twice`: the C# recording made afresh in WSL beside the Rust one
and the Rust one again, three stacks at once.

| Comparison | Structural | Bytes |
| --- | --- | --- |
| C# against Rust | 786 unexplained, 1,026 explained, 25 identical (the G2 box record's numbers) | not run |
| Rust against Rust | 1,836 identical, 1 unexplained | 1,836 identical, 1 unexplained |

The one Rust against Rust diff is `19-health/plant-wait`: its `search3` was answered while
Navidrome was still giving the planted albums their covers after `plant-scan-done` saw the
scan end (one recording has `coverArt` on the albums, the other not, and the two albums
in another order). It showed with five stacks and port agents' builds going at once; the
sharded run below with `--rust-twice` had it 0. A corpus fix (wait for the planted albums'
`coverArt`) is for whoever owns the corpus.

| Run (WSL) | Stacks at once | Wall time |
| --- | --- | --- |
| `rust/main` `--rust-twice`, C# not cached | 3 (C# 741 s, Rust 3,087 s, Rust again 3,088 s) | 51 min run, 55 min with the musl compile |
| `port/g2-bd` (`eb59c77`) beside it and `port/g2-coq` | 1 each (they waited for, then shared, the C# recording `rust/main` was making) | 47 and 52 min run; 684 and 786 unexplained |
| A trial four-shard plan, C# not cached, Rust waits from C# | 4 C# shards, then each Rust shard after its C# one | 28 min |
| The same, C# cached, `--rust-twice` | 8 Rust shards | 17 min; Rust against Rust 0 in both modes |

On the box the same `rust/main --rust-twice` is about 11 min of C# (when not cached) and two
45 to 55 min Rust recordings one after the other, about 2 hours, after the wait for the box
lock (seven runs were queued for it while these ran). The trial plan was a timing test with
the shards work in progress; its C# against Rust count (795) is not yet the whole corpus's.
The longest shard (15-radio to 18-dashboard) took 707 s against C# and about 1,000 s against
Rust, so that shard sets the time of a sharded run.

## Gap groups

Sizes from the migration plan § 2 (C# logic and test lines added, test
methods). "Diffs now" is the latest "own" count (requests tagged with the
group that differ from C# 2026.10.05): the G1 J and I record above (the G2 C, O
and Q record for those three), proposed
intentional diffs counted as differing. Status: open, in progress, done, or no
Rust change.

| Group | Commits | C# lines | Test methods | Agent-days | Diffs now | Status |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| A Last.fm search cleanup polish | 1 | 40 | 5 | 0.25 | 0 | done |
| B Download matching | 8 | 644 | 46 | 0.75 | 11 | open (`0a700c0` done) |
| C Tagging polish | 4 | 465 | 24 | 1.0 | 9 | done |
| D Download log, drawer, Find songs picks | 13 | 2,146 | 53 | 1.5 | 110 | open |
| E slskd sharing, Share switch | 8 | 1,084 | 29 | 1.0 | 4 | done (G2, `port/g2-eg`) |
| F Library edits, delete, trash, put back | 10 | 1,345 | 35 | 1.25 | 128 | open |
| G Top songs, charts, Popular right now | 4 | 1,472 | 35 | 1.0 | 48 | open, owned by `port/g-charts` |
| H Spotify import, trickle | 4 | 3,197 | 69 | 2.0 | 202 | open |
| I Secret masking in the admin API | 3 | 207 | 7 | 0.25 | 3 | done (G1) |
| J Admin sign-in | 6 | 921 | 29 | 1.25 | 6 | done (G1) |
| K Lidarr stale guid, bare address 302 | 2 | 82 | 4 | 0.25 | 0 | done |
| L Multi-source radio | 16 | 2,976 | 127 | 2.0 | 18 | done (G3) |
| M octo-sonic and the shim's `/ytm` | 3 | 22 | 3 | 0 | 0 | no Rust change (own images) |
| N Afterglow, activity view, toasts | 8 | 572 | 18 | 0.5 | 14 | open |
| O Search album ownership | 1 | 808 | 23 | 0.5 | 17 | done |
| P Covers keep-or-replace, explicit marks | 4 | 1,739 | 50 | 1.25 | 68 | open |
| Q Made for you lists | 3 | 887 | 26 | 0.75 | 5 | done |
| R Library health on the server | 5 | 3,130 | 39 | 2.0 | 97 | open |
| S Estimated stream length fix | 1 | 153 | 12 | 0.5 | 0 | done |
| T Starter stack, install, CI, images, updater | 15 | 10 | 0 | 1.5 | 0 | release phase |
| U Shared case files, timing tests | 2 | 0 | 0 | 0 | 0 | no Rust change (Rust reads the files) |

Port waves, from the plan: G1 is the settings and env vars for every group,
the 7 new state files with fixtures, then J, I, A, K, S. G2 is B, C, D, E, G,
O, Q. G3 is F, H, L, P. G4 is R and N.

Things the gap brings besides code: 7 new state files
(`explicit-backfill.json`, `imports.json`, `imports-trickle.json`,
`radio-outcomes.json`, `sonic-features.json`, `spotify-accounts.json`,
`tag-edits.json`) plus `admin-recovery-code`; new fields in
`downloads-history.json`, the trash notes and `upgrades.json`; 34 new env
vars; the `zip` crate for imports.

**Done in wave G1 (branch `port/g1-core`):** every 2026.10.05 setting and
env var (config.md §3.6, §3.12, §3.15 to §3.18), bound and clamped, and
listed by GET settings, raw-config and config-sources. The 7 new state
files and `admin-recovery-code`, read and written as the C# stores did, each
with a regenerated fixture and a round trip (state-files.md §4.26 on). The
new fields in the files Octo already kept, including three the list above
missed: `external-ids.json` (`ExplicitContent`, `Version`, and a clean
edit's own id), `generated-playlists.json` (`ForYou`, `ForYouCovers`) and
`library-actions.json` (`Copy`, `DownloadAllowed`, `BlocksDownloads`, state
`Restored`); `downloads-history.json` is now written without nulls, as the C#
writer does. What the later groups find waiting for them:

| Group | Already there |
| --- | --- |
| B, D | `DownloadHistoryEntry.key` and `log` (`octo_core::models::download::{AcquisitionEvent, AcquisitionCandidate}`, with `Quality`); `PickedCopy` (`services::common::download_picks`) and `UpgradeJob.pick`. `base_download_service` records `key: None` (STUB(D)). `GET /api/admin/downloads` already drops `log` and says `hasLog`. |
| E | `Soulseek.WebUrl` (`safe_web_url`), `CheckListenPort`, `ShareLibrary`, `SharedFolders` (`shared_folder_list`); the save's WebUrl check and the slskd password rules (9f9502d's admin side). |
| F | `services::library::library_tag_edits` (`TagEditEntry`, the file); `LibraryActionEntry.copy`, `download_allowed`, `blocks_downloads()` (the journal's never-requested check uses it); `LibraryActionState::Restored`; `QuarantineManifest.sidecars` and `mappings` (written null, STUB(F)). |
| G, Q | `GeneratedPlaylists.PopularNow`, the Made for you switches and clamps (`any_for_you`); `UserMixes.for_you` and `for_you_covers` (filled by Q in G2). |
| H | `Imports` settings; `services::imports` (`ImportList`, `ImportTrack`, `TrickleJob`, the three files' load and save). |
| L | `RadioSources` settings (`effective_weight`); `services::radio::radio_outcome_store` and `services::sonic::sonic_store`; `LastFmRadioBan`; the status page's octo-sonic probe. |
| P | `SoulseekRouting.explicit_content` and `version`, the clean id rule and the explicit mark kept across registrations; `services::library::explicit_backfill_store`. |
| H, N, R, D | Their controllers call `BrowseSessionStore::navidrome_user_of` (never the recovery code) and read who called from the gate's `AdminCaller` extension (`middleware::admin_sign_in_gate::{Caller, caller_of}`), as `LibraryHealthController` and `AdminDownloadsController` did. |
| Every group with a new route that reads or changes library files | Add it to `middleware::admin_sign_in_gate::LIBRARY_FILE_ROUTES` (with the words the C# endpoint answered), so the gate refuses the recovery code there as on the rest; the walk test lists the routes. |

## Commits by group

### A. Last.fm search cleanup polish

- [x] `6f90ce1` fix(lastfm): the rest of the mislabelled search rows, and a gap inside "Connected as"

### B. Download matching (versions, mash-ups, wider star search, format rename, relay file name)

- [ ] `15087f6` fix(soulseek): a star searches as wide as Better quality, and wider again when the limit fills
- [ ] `ff46dde` fix(downloads): a song's version tag is not words its file must carry, and the song itself never takes an edit
- [ ] `39770ed` fix(downloads): a guest credit inside a version tag, "(Radio Edit - feat. X)", no longer makes the version words a file must carry
- [ ] `066ea88` fix(downloads): a drumless, piano, 8D or other performance edition is a version, while deluxe and anniversary editions stay the same recording
- [ ] `53731d2` fix(downloads): a mash-up, medley or megamix never passes as the song, and only a file named as just the song is taken for its radio edit
- [ ] `85bba0f` feat(downloads): a file named for another format is renamed to the one it holds before tagging
- [x] `0a700c0` fix(relay): a download keeps the file name Navidrome gave it
  (ported with S: `StreamLengthTests` checks it)
- [ ] `a38487d` docs(readme): a download named for the wrong format is renamed before tagging

### C. Tagging polish (one album's facts, explicit advisory, Try it)

- [x] `2463c23` feat(tagging): a download keeps only its own album's facts, one value per field, and says whether it is explicit
- [x] `67f56fa` docs(readme): what a download loses from a peer's tags, DISCTOTAL and the explicit advisory
- [x] `099fdd3` feat(tagging): Try it on a song shows whether the song is explicit and what a download would take off the file
- [x] `a60f06c` docs(readme): Try it on a song shows the advisory and what a download would take off
  (the `LogStep` lines `02cc06b` added to these paths are D's; the cover chain's `CoversOnHand` is
  P's, and C passes no embedded art for a song filed elsewhere, as `2463c23` did)

### D. Download log, drawer, Find songs picks

- [ ] `9ac4e5a` feat(downloads): every download keeps a log, and Find songs picks the copy to fetch
  (its Lidarr half is on `rust/main` with K: `LidarrRelease`, `LidarrReleasePick`,
  `list_releases`, `grab_release`, the grab in `start_album_search`, the request's `release`,
  and its two `LidarrClientTests`; D still wires `SongFinder`, `DownloadPicks` and the
  download log's `step` to them)
- [ ] `6113fec` fix(downloads): a picked copy waiting in the upgrade queue is saved without its derived fields
- [ ] `1469b8c` docs(readme): the downloads drawer, Find songs and their endpoints
- [ ] `02cc06b` feat(downloads): a download's log says what came off the peer's tags and whether the song is explicit
- [ ] `11900f1` fix(downloads): Find songs picks a copy by its own id, and only once the search has ended
- [ ] `c3e80bb` fix(downloads): a picked copy meets the deny list, and in place of a library file the search's own checks
- [ ] `25b1d67` fix(downloads): a second pick before the upgrade starts replaces the first, and otherwise says another copy is on its way
- [ ] `e24eca1` fix(downloads): a pick is let go when its download ends, so a later download never takes it
- [ ] `45f4463` fix(downloads): Find songs runs two looks a person at once, never drops a running one, and a library song's look is for the allowed list
- [ ] `57ab90d` fix(downloads): Find songs keeps a few passed-over files of each reason on a popular song's list, and its fit count matches the rows
- [ ] `13ec9f9` fix(downloads): Find songs says how many fitting copies its list shows when it has no room for all of them
- [ ] `696cbff` fix(downloads): a shared download's log names nobody, and import errors show no server paths
- [ ] `7602787` feat(downloads): keep each finished download's log with its fetched-songs entry

### E. slskd sharing and the Share switch

- **Fix in port:** the Soulseek web address (`Soulseek.WebUrl`) is saved with
  the spaces it was typed with (BACKLOG "Found while building the harness");
  save it trimmed, with a Rust test and an intentional diff. Done in G2
  (`fc00777`): saved and read trimmed, tests `slskds_web_address_is_saved_without_spaces`
  and `slskds_web_address_is_read_without_spaces`, proposed intentional diff
  `slskd-web-url-trimmed`.

Done in G2 (`port/g2-eg`): `octo_core::soulseek::{soulseek_sharing, soulseek_share_switch}`
(the card's records and slskd's answers, the slskd.yml block), `services::soulseek::{SoulseekSharing,
SoulseekPortCheck, SoulseekShareSwitch}` (the switch runs as the `SoulseekShareSwitch` worker),
the slskd client's reads, rescan, settings file, scan cancel and upload cancel, and
`controllers::admin::soulseek_sharing` (`GET /api/admin/soulseek/sharing`, `test-port`,
`rescan`, `share`). The four compose, installer and README commits change no Rust.

- [x] `9f9502d` fix(admin): the slskd password never goes back to the browser (ported in G1 with group I)
- [x] `9624b57` feat(soulseek): slskd shares the music library back, read-only, with polite upload limits
  (compose, `.env.example`, install.sh: no Rust change; the settings came in G1)
- [x] `0abab4f` feat(soulseek): a Sharing card and an Open slskd button on the Soulseek page
- [x] `c785401` docs(readme): sharing back on Soulseek, the port to forward, and Open slskd (no Rust change)
- [x] `e3a5063` feat(soulseek): sharing is a choice, on for new installs and never switched on by an update
  (compose, `.env.example`, install.sh: no Rust change; `ShareLibrary` defaults off since G1)
- [x] `416644b` feat(soulseek): a Share my library switch on the Sharing card, applied to slskd within seconds
- [x] `e30b242` docs(readme): the Share my library switch, and how it reaches slskd (no Rust change)
- [x] `d2114a6` test(soulseek): the stock slskd.yml sample has Linux line ends however it was checked out

### F. Library edits from the apps, delete, trash, put back

- **Fix in port:** clearing an ISRC does nothing on disk
  (`TagWriterExtras.cs:98-101` returns on an empty value), so undoing an ISRC
  retag answers `isrc: null` and leaves the tag in the file; remove the tag
  (group C's tag writing is the same code), with a Rust test and an
  intentional diff. Done in G2 with C: the shared tag writer clears it
  (`tag_writer_extras::set_or_clear_text`, test
  `clearing_an_isrc_takes_it_off_the_file`), and the proposed intentional diff
  is `isrc-clear-takes-the-tag-off`; F's tag edits only need to call it.

- [ ] `374dac8` feat(library): Library health fixes from the apps: tags written in place, albums joined, covers added, and a removed song put back
  (G2: `LibraryTagEdits.Set` must write through `tag_writer_extras::set_or_clear_text`, so a
  cleared field, an ISRC above all, leaves the file; the port-only fix and its intentional diff
  `isrc-clear-takes-the-tag-off` are in place)
- [ ] `8606390` test(library): joining a split album takes away a peer's stray album id, release date and version
- [ ] `15e3bb5` fix(library): removing a second copy leaves the song wanted, so the kept copy can still be upgraded
- [ ] `433acff` fix(library): Delete from disk takes the song's own lyrics files to the trash with it, and Put back returns them
- [ ] `a4238fa` fix(library): Put back links the outside song to its file again, as it was before the song was removed
- [ ] `3204161` feat(admin): Allow downloading again on the Library actions history, and a removed song's log says how to get it back
- [ ] `6b6b61d` fix(library): a removed song's outside link is found by its whole path, and the trash note's link is tested
- [ ] `f2c6a65` fix(library): getLibraryActions offers remove only to a Navidrome admin
- [ ] `a0fcecb` test(library): joining a song onto an album filed under another album artist takes that album artist, keeps its own artist, and undoes
- [ ] `ed0bd7c` fix(library): a library song's length is read from Navidrome's fractional seconds instead of always missing

### G. Top songs, charts, Popular right now

Owned by `port/g-charts` (the Apple Music charts plan, which ports G generalized into chart
playlists); left out of `port/g2-eg` on purpose.

- [ ] `3e35798` feat(search): an artist's top songs and the chart of the moment, ranked and marked in the library
  (G2: `TopSongsService.CatalogSong` is `services::library::for_you_lists::catalog_song`, marked
  `STUB(G)`; New Releases shares it, so `TopSongsService` calls it or replaces it there)
- [ ] `3d5bea4` feat(top-songs): every app's getTopSongs ranked by Octo, the caller's songs as their own
- [ ] `43d8325` feat(lists): Popular right now, the chart as a playlist in every app
- [ ] `2397369` docs(lists): US spelling in a Popular right now comment

### H. Spotify import and the trickle

- **Fix in port:** "Keep as playlist" answers 500 against Navidrome 0.64.2
  (Octo reads Navidrome's answer to the playlist create as JSON; likely the
  `application/json; charset=utf-8` content type makes Navidrome take the
  body as an M3U import), and every 30 s retry leaves another empty playlist
  named after the time; create the playlist the way Navidrome reads it and
  never retry into a new empty one, with a Rust test and an intentional diff.

- [ ] `4008358` feat(imports): Spotify lists matched against the library, kept as playlists, and a trickle that fetches the rest
- [ ] `ef24d3d` feat(admin): a Spotify import page for connecting, lists, playlists and the trickle
- [ ] `1e8cf04` fix(install): a reinstall keeps the Spotify import settings it never asks about
- [ ] `45f730c` fix(imports): a file holds at most 50,000 songs, a zip 200 files and 256 MB unpacked, and a person 500 lists

### I. Secret masking in the admin API

Done in G1 (`controllers::admin::secrets`). `d7e8482` is the README.

- [x] `031e563` fix(admin): every key, token and webhook address in the settings reads as the placeholder, and saving it back keeps the stored one
- [x] `9210c63` fix(admin): a saved password, key or token never follows a server address it was not typed again for
- [x] `d7e8482` docs(readme): saved credentials never come back out of the admin API

### J. Admin sign-in (gate, recovery code, role check, ADMIN_PORT)

Done in G1. `3b82267` is the dashboard (`octo/wwwroot`, served as is by both
builds) and `7a9f753` the README, compose, `.env.example` and installer: no
Rust change beyond the settings they name.

- [x] `a4333f0` feat(admin): the admin API needs a sign-in with a Navidrome admin account
- [x] `3b82267` feat(admin): a sign-in screen in front of the whole dashboard
- [x] `7a9f753` docs: signing in to the dashboard, the recovery code, and scripts with a cookie jar
- [x] `2eb2299` fix(admin): the hourly role check reads Navidrome's user list, since getUser only answers for yourself
- [x] `06e9a60` fix(admin): a demoted admin is signed out even when Octo's own Navidrome token came from them
- [x] `f591793` fix(admin): Navidrome refuses its user list to a non-admin with 403, and that now means Octo's token is stale

### K. Lidarr stale guid, bare address 302 (#80)

- [x] `e3aed24` fix(lidarr): a picked release Lidarr forgot is found again by its title, and a pick an album search already covers says it was not grabbed
  (the Lidarr client and track fetcher; `PickedCopy.Size`, `SongFinder`'s size and the
  Soulseek service's `Step` belong to D's picks and come with them)
- [x] `4a329aa` fix(proxy): the bare server address sends to the web app, as Navidrome does (#80)

### L. Multi-source radio (#78, sources, blend, learning, bans, Sounds alike client)

- **Fix in port:** an outside song's registry id ignores its album
  (`ExternalIdRegistry.cs`, `MakeShortId`): radio registers songs from
  Last.fm without an album and replaces the search's entry, so later answers
  lose the album; keep the album a registration already had (as the length,
  ISRC and explicit mark are kept), with a Rust test and an intentional diff.
  Done in G3 (`external_id_registry::register`, test
  `a_registration_without_an_album_keeps_the_album_already_known`, known-diffs
  "An outside song's album in the registry"; no corpus request reaches it, so it has no
  intentional-diffs entry, see the G3 record).

Done in G3 on `port/g3-l`. The pure parts are `octo_core::radio` (`RadioProvider`,
`RadioTypes`, `RadioBlend`) and `octo_core::last_fm::radio_filler`; the sources, the set,
song radio and the outcome store are `octo::services::radio`; octo-sonic's client, store and
analysis worker `octo::services::sonic`; `YouTubeMusicClient` and `ListenBrainzRadioClient`
beside their services. `SongRadioService` is shared, with each request's resolver handed to
`build`. `TopSongsService.LibraryQuery` (7a8cc5f's library search) lives in
`last_fm_radio_track_resolver::library_query`, marked STUB(G): G's `TopSongsService` takes it
over. `6a119db`'s settings views and the octo-sonic status card came with G1. The station
previews' em dash Brandon saw is in the dashboard (`octo/wwwroot/admin/admin.js`, the Radio
page's station grid joins artist and title with one), which both builds serve as is; no Rust
radio code builds a string with one.

- [x] `446d393` feat(radio): a song Last.fm cannot place is a radio of its own album and genre (#78)
- [x] `02218c1` feat(radio): one star keeps a song off radio, filler left out, a cached answer never short-changes a bigger ask
- [x] `f621926` fix(radio): British-spelled bookkeeping tags are not station genres again
- [x] `8f5142d` refactor(radio): song radio asks a set of sources and blends them; Last.fm is the first
- [x] `c1d701d` feat(radio): every radio song says which source suggested it (octoSuggestedBy, octoRadioSources)
- [x] `8346a30` feat(radio): YouTube Music's radio is a source, and a song it suggests plays the video it named
- [x] `7b0e226` feat(radio): ListenBrainz's similar recordings are a source, and LB Radio fills artist and genre stations when Octo has a token
- [x] `6a119db` feat(admin): radio sources and their weights on the Radio page
- [x] `2d04f3f` feat(radio): Sounds alike, library songs that sound like the seed, from Octo's own analysis
- [x] `7ba1123` feat(radio): stations draw on every radio source and remember which one suggested each song
- [x] `ece2795` feat(radio): radio learns which sources each listener keeps, and weighs them by it
- [x] `7a8cc5f` fix(radio): song radio asks fairly and never keeps a failure as an answer
- [x] `b446983` fix(radio): stations share per seed and learning counts each serving once
- [x] `3de8dde` fix(sonic): the analysis says it is about to start, not that it is off, until its first look
- [x] `e4b0327` fix(sonic): Sounds alike never forgets songs on a bad listing, and octo-sonic runs locked down
- [x] `7af692c` fix(sonic): the dashboard and comments spell analyzed the US way

### M. octo-sonic (Rust) and the shim's /ytm (Python)

No Rust server change: both are their own images.

- [ ] `0488476` feat(sonic): octo-sonic reads a song and says how it sounds (bliss-rs)
- [ ] `b900557` feat(shim): YouTube Music search and radio, and every shim test runs in CI
- [ ] `9ea267a` feat(sonic): octo-sonic starts with the stack, and RADIO_SOUNDS_ALIKE=false turns it off

### N. Afterglow dashboard, activity view, toasts

- [ ] `77701bc` feat(admin): the dashboard's light comes from the fetched songs' covers, read with the station covers' colour rules
- [ ] `31e3715` feat(admin): Afterglow, the dashboard lit by a fetched song's cover
- [ ] `32d05af` fix(admin): Recently added offers one song per cover, so an album fetched whole is one choice of light
- [ ] `5a00fb1` fix(admin): choosing the newest fetched song lights the page with it, and a faintly tinted cover lights it quietly
- [ ] `2e4fdcf` fix(admin): Octo's own service card carries the octopus
- [ ] `d52ae54` feat(admin): Fetched songs in the Afterglow look
- [ ] `ca47988` feat(admin): one activity view of everything Octo is working on
- [ ] `af48eb1` feat(admin): live progress toasts for Octo's background work

### O. Search album ownership

- [x] `f49fb80` fix(search): an outside album the library holds is listed as the library's, and one held in part says how much

### P. Covers keep-or-replace, explicit marks and backfill

- **Fix in port:** resuming a cancelled cover run counts `kept` and
  `albumsDone` again ("6 of 3 albums", `CoverUpgrade.cs:696,744`); count each
  album once, with a Rust test and an intentional diff.
- **Fix in port:** the cover scan walks into `/music/.octo-trash` (and the
  album id of a trashed song depends on the dated folder); leave the trash
  folder out of the scan (group F owns the trash), with a Rust test and an
  intentional diff.
- The recovery-code explicit marks are already refused by the gate (G1).

- [ ] `5cea252` fix(covers): a download keeps the cover it came with when it is the same art and big enough
- [ ] `86ad6b8` fix(covers): every strict source is asked, and only a copy of the release's own art can win
- [ ] `4f2aeb6` feat(explicit): outside songs and albums say explicit or clean, and a clean copy has an id of its own
- [ ] `d80141f` feat(explicit): mark the songs already in the library explicit or clean

### Q. Made for you lists

- [x] `f54467a` feat(lists): Made for you, three lists per listener from their own plays
- [x] `1f940f2` fix(lists): only artists a listener played or hearted are theirs, and a shared song filed under their artist is their copy
- [x] `452f47e` fix(lists): Check Mode on Made for you
  (the Made for you settings came from g1-core's `generated_playlist.rs` as it wrote them, with its
  admin settings and raw-config rows, so the two merge cleanly)
- [x] Lists off in the dashboard (2026-10-08, `fix/q-lists-off`): the parity run's
  `16-lists/listener-playlists-lists-off-json` kept New Releases. The getPlaylists right after
  the save came in before the file watcher read settings.json again (250 ms later), so it
  answered from the settings before the save. With Popular right now's chart calls that one
  request took 15 s, the whole wait, so no second ask came. A settings save and a Raw config
  save now read the file again before they answer, and the next request sees the save.
  Rediscover, Deep Cuts and the list settings (weeks, artists, months) had the same lag and the
  same fix. C# still waits for its watcher; a request inside those 250 ms is the only place the
  two can answer differently.

### R. Library health on the server, dashboard parity (logs, Find songs, Recently removed)

- [ ] `69f577f` feat(health): Library health findings and fixes on the server
  (G2: `LibrarySnapshot` and `LibraryAlbumIndex` are O's, at `f49fb80`; R adds `ReadAtUtc`,
  `RefreshAsync`, `Rows` and the song row's health fields)
- [ ] `e36d823` fix(health): check songs in the order the apps read them
- [ ] `c156710` feat(admin): Library health page on the dashboard
- [ ] `5d3f096` feat(admin): download logs, Find songs and Recently removed for the dashboard
- [ ] `eef5fbd` feat(admin): download logs, Find songs and Recently removed on the dashboard

### S. Estimated stream length fix

- [x] `67f3c93` fix(stream): a transcode's estimated length is never promised to the client

### T. Starter stack, install.sh, README, CI, sidecar images, updater

Done in the release phase (plan § 9): the compose file, install.sh and the update helper move to the Rust image there.

- [ ] `73b83c4` feat: a starter stack with Navidrome, and a README that leads with the music service
- [ ] `6a2a477` docs(readme): say the best copy Octo can find, not lossless, since a kept song can be FLAC, MP3 or Opus
- [ ] `90df881` fix(install): a terminal that cannot clear the screen no longer ends the installer before it prints a word
- [ ] `d1a7e67` fix(install): defaults apply on a fresh install, so pressing Enter no longer makes the Octo folder the music folder
- [ ] `aba1743` fix(compose): the starter Navidrome is navidrome-starter, so a Navidrome of your own named navidrome in an override no longer merges with it and leaves Octo's dependency undefined
- [ ] `76684ab` fix(install): a re-run keeps everything in .env, and a first install gets the defaults it shows
- [ ] `b2e5d9c` ci(images): publish the yt-dlp shim beside Octo, and pull it on image installs
- [ ] `610dba2` feat(images): updates pull the yt-dlp shim and octo-sonic instead of compiling them
- [ ] `b2298f5` ci(images): native arm64 builds, latest on releases, and the release waits for its images
- [ ] `94e30d1` ci(images): the release waits until every image can be pulled without signing in
- [ ] `0d81d0b` ci(docker): octo's digests are collected apart from octo-sonic's
- [ ] `c038b87` ci(docker): the Sounds alike image is ghcr.io/winters27/octo-sonic
- [ ] `25ad530` docs(readme): a 20-second demo at the top, searching, playing and keeping a song
- [ ] `0adc6b3` release: 2026.10.05
- [ ] `ce63143` docs(readme): the apps' links point at 1.5.0 [skip ci]

### U. Shared case files, timing tests

The Rust tests read the shared case files as they are.

- [ ] `aae574a` test(identity): the shared song cases gain drumless and other editions, karaoke in the style of, and mash-ups, as the apps have them
- [ ] `97855f1` test: two timing tests wait for what they check instead of sleeping

## Improvements we want while porting

The port does not have to stay one to one. Each of these is a known diff with
its reason when it changes an answer.

- [x] **The admin gate as one router layer, default deny.** Axum has no
  endpoint attributes to read, so the gate is a layer with an explicit list of
  open routes; an unknown `/api/admin/<x>` answers 401 too (group J). Done in
  G1 (`middleware::admin_sign_in_gate`). No answer changes: the C# gate also
  answered 401 to an unknown admin path (its catch-all endpoint carries no
  `[AdminOpen]`), so it needs no intentional-diff entry; a test walks every
  admin route with four methods.
- [x] **The signed-out status cache** (C# caches it 10 s). Under load the Rust
  `/api/admin/status` is slower than C# only because it probes five services
  on every call (group J). Done in G1, as the C# does it (ten seconds, one
  probe round at a time).
- [ ] **Plain error words instead of .NET's.** The port copies .NET exception
  texts the apps can show ("The request was canceled due to the configured
  HttpClient.Timeout of 100 seconds elapsing.", "Connection refused
  (host:port)"). Say what happened in plain words, in both builds while the
  twin rule holds.
- [ ] **Em dashes in answers.** Two messages shared with the C# app still
  carry one (the "absolute URL" hint in `subsonic_proxy_service.rs`, the
  music folder warning in `navidrome_identity_service.rs`). Change them in
  both builds together.
- [ ] **US spelling in identifiers.** Comments and the port notes are done;
  names like `cover_colours`, `COLOURLESS` and `normalise_*` are not. Rename
  inside the code only, never a stored key or a JSON field.
- [ ] **`.unwrap()` outside tests** (about 92 in non-test files): turn the
  reachable ones into errors that are logged and answered.
- [ ] **Module names from the port's waves** (`tests_6a1`, `helpers_6b1`,
  `test_support_6a2` and 13 more): name them for what they hold.
- [ ] **Smaller list cover JPEGs.** The Rust encoder writes 4:4:4 (17.6 KB
  against C#'s 11.8 KB on the 600 px placeholder); 4:2:0 should match the
  size with no visible change.
- [ ] **Cut the .NET compatibility helpers where tests prove the same
  answers** (`octo_core::common::dotnet`, the `XElement` replica), after the
  cutover, behind the tests.
- [ ] **Split the `octo` crate** only if the loop gets slow again: today an
  edit there rebuilds the tests in 12 s.
- [ ] **A `VERSION` file at the cutover.** `build.rs` reads it first; until
  then it reads `octo/octo.csproj`, so a release bump is one edit for both
  builds.
- [ ] **Move the shared files beside the crates at the cutover** (dashboard,
  Assets, cover design, appsettings, golden samples) and drop the `../../octo`
  paths.
- [ ] **cargo-deny's two ignored advisories** (`paste` under lofty,
  `ttf-parser` under cosmic-text): drop each when its parent moves on.

## Before the port can be gated in CI

- Push `rust/main` so `rust.yml` and `ci.yml` run, then one manual run of
  `docker-rust.yml`; record the times and the cache sizes above. The
  `octo-rust` package starts private on ghcr.io. (Nothing is pushed until
  Brandon says so.)
- The test box: LXC 131 had 11 GB free on its disk after the phase 1 runs (the
  parity builder's cache, about 6 GB at most, and one Rust image of 1 GB are
  kept). The Proxmox host is the tight part: it had 4 to 5 GB of memory free,
  and a run pushed it into swap once (see `parity/README.md`); `lxc131.sh`
  checks the host before it starts and stops the run if the host runs short.
- The bridge release (C#, Octo pulled not built, update helper v2) ships
  together with the Rust release, not before (Brain, "MISTAKE AND UNDO").
