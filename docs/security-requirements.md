# Security requirements and baseline audit

What every new Octo module must meet, and what the existing code gets wrong today. Written for
FLA-9 on 2026-10-10 against `origin/main` at `aac1c7a` (release 2026.10.05). `AGENTS.md` makes
the Security agent review every PR that touches auth, user data, credentials, external input
or file deletion; this page is the checklist that review uses (FLA-27 uses it for the final
pass).

Part 1 is the requirements. Part 2 is the baseline audit: scans, history check, review
findings, each with a severity, a `file:line` and a proposed fix. No exploit detail is given on
purpose; the repository is public.

Severity: **Critical** (exploitable now, no sign-in, big impact), **High** (serious, needs a
plausible condition), **Medium** (weakens a defence or needs an attacker already near), **Low**
(hardening).

---

## Part 1. Requirements for new modules

Each item is written so a reviewer can answer yes or no. "R" ids are stable: cite them in PRs.

### 1. Secrets handling

- **R1.1** No secret (API key, password, token, client secret, recovery code) is committed.
  New settings go in `.env.example` with an empty or obviously fake value, never a real one.
- **R1.2** A secret is never written to a log, an exception message, a notification, or an HTTP
  answer. Logs may say that a secret is set, never what it is. (Baseline exception: SEC-5.)
- **R1.3** A secret setting shown on the dashboard goes through `MaskSecret` in
  `AdminController` (the pattern already used for `Soulseek.Password`, `Lidarr.ApiKey`).
  Saving a masked value back must not overwrite the real one.
- **R1.4** A file that holds secrets (tokens, keys, codes) is written by a temporary file
  created with mode `0600` from the start (`UnixFileMode` on `FileStreamOptions`, or
  `File.CreateText` after setting the mode), then moved over the target. Setting the mode after
  the write leaves a window and, on failure, a readable `.tmp`. Failure to restrict the mode is
  logged and must not silently continue for token files.
- **R1.5** Tokens a user hands Octo (Spotify, Last.fm session keys, anything new) are stored
  apart from `settings.json`, keyed by user, and removed when the user disconnects. Prefer the
  shortest-lived token that does the job; never store a password when a token will do.
- **R1.6** A secret that must be compared (codes, tokens) is compared with
  `CryptographicOperations.FixedTimeEquals`, and generated with `RandomNumberGenerator`.
- **R1.7** Secrets reach the container through `.env` -> `docker-compose.yml` environment (the
  existing pattern). `.env` stays `chmod 600` (done by `install.sh`). No secret in an image,
  a build arg, or a URL query string that Octo itself builds for a browser.
- **R1.8** Default credentials are not allowed in anything a user can start with `docker compose
  up`. A new service either has no default password (it refuses to start) or one the installer
  generates. (Baseline exception: SEC-2.)

### 2. Per-user data isolation

Octo holds per-person data (Spotify tokens, imported lists, hearts, radio learning, playlists)
for people who share one server. Rule of thumb: the identity comes from a verified sign-in,
never from a value the caller types.

- **R2.1** The user a module acts for is taken from a source Navidrome has accepted: the
  dashboard session (`BrowseSessionStore.NavidromeUserOf`) or a Subsonic request whose
  credentials passed the upstream check (see `ImportsSubsonicController.CallerAsync`). Never
  from a request field, header or query value on its own.
- **R2.2** Every read and write of per-user data takes the user as a parameter and filters on
  it in the store (`ImportStore.Get(owner, id)` is the model). A store method that returns "all
  users' rows" is marked, internal to background work, and not reachable from a controller.
- **R2.3** An id from a request (list id, playlist id, job key) is looked up inside the caller's
  own rows. "Not yours" and "does not exist" give the same answer, so ids cannot be probed.
- **R2.4** A background job that acts for a user (trickle fetch, playlist sync) stores the owner
  with the job and never infers it later. When it acts through an admin identity (as
  `ImportPlaylists` does to create a playlist and hand it over), it states why in a comment and
  hands ownership to the user.
- **R2.5** Playlist, favourite and rating changes go to Navidrome with the caller's own
  credentials, so Navidrome's ownership checks apply. Octo does not add an admin-identity
  shortcut around them.
- **R2.6** OAuth-style callbacks that run without a sign-in (the Spotify callback) carry an
  unguessable single-use `state` bound to the user and a short expiry (15 minutes today), and
  use PKCE. A new callback copies that.
- **R2.7** Each module with per-user data has a test that signs in as user A, creates data, then
  as user B tries to read, change and delete it by id, and expects a refusal each time.
- **R2.8** One user's data is not placed in anything another user can read: shared playlists,
  shared notifications, logs shown on the dashboard, the Soulseek share.

### 3. Admin authentication

- **R3.1** Every admin endpoint is under `/api/admin`, so `AdminSignInGate` covers it. A new
  controller never adds its own admin path elsewhere, and an endpoint marked `[AdminOpen]` must
  say in a comment what an unauthenticated caller can learn (today: sign-in itself, session
  state, and a status answer that reveals only up or down).
- **R3.2** State-changing admin calls (POST/PUT/PATCH/DELETE) rely on `AdminRequestGuard` (the
  `X-Octo-Admin` header and no CORS). A module does not add CORS to `/api/admin`, does not
  accept state changes on GET, and does not read the session token from the query string.
- **R3.3** A call that touches library files or deletes anything requires a Navidrome admin
  session, not a recovery-code session (`NavidromeUserOf` returns null for recovery).
- **R3.4** Any new place that checks a password or code is rate-limited with `SignInThrottle`
  (or a successor), counts failures, and answers 429 with `Retry-After`.
- **R3.5** Admin cookies stay `HttpOnly`, `SameSite=Strict`, `Path=/api/admin`, and `Secure` over
  HTTPS. Modules do not set their own auth cookies.
- **R3.6** Admin answers carry `Cache-Control: no-store` unless an endpoint says otherwise
  (`UseAdminSecurityHeaders` does this); modules that return secrets or per-user data must not
  override it.
- **R3.7** Nothing new turns the sign-in off or adds a bypass. `ADMIN_SIGN_IN=off` stays the only
  one and logs a warning at start-up.
- **R3.8** Ports: a new service publishes no host port unless users must reach it from outside
  the compose network. If it must, it binds to `127.0.0.1` by default, with an `.env` setting to
  widen it, and that is documented in the README.

### 4. Input handling toward the yt-dlp shim and Soulseek

Input here comes from the open internet (search results, peer file names, tags, remote
metadata), from users, and from other containers.

**yt-dlp shim** (`yt-dlp-shim/app.py`)

- **R4.1** Anything that becomes a process argument is passed as a list element, never through a
  shell, and can never start with `-` (so it cannot become a yt-dlp or ffmpeg option). Free text
  goes only behind a fixed prefix such as `ytsearch1:`.
- **R4.2** A YouTube video id is validated against `^[A-Za-z0-9_-]{11}$` at the shim's edge, before
  it is cached, logged or put in a URL or path. (Baseline gap: SEC-8.)
- **R4.3** A path sent to the shim is canonicalised and checked against the download root with a
  separator boundary (the `/download` check does this) and is escaped for yt-dlp's `%(...)s`
  output template.
- **R4.4** The shim is reachable only on the compose network. No `ports:` entry for it, and no
  endpoint that can read a file outside the download root.
- **R4.5** Every call has a timeout, an output size limit, and runs under the concurrency gate.
- **R4.6** Metadata from YouTube (titles, artists) is data: it is never used as a path, a command
  or a log format string without sanitising.

**Soulseek / slskd** (`octo/Services/Soulseek`)

- **R4.7** A file or folder name from a peer is never used as a path directly. Take
  `Path.GetFileName`, run it through `PathHelper.SanitizeFileName`/`SanitizeFolderName`, and
  confirm the final full path is inside the intended folder (`NavidromeSongPathResolver.IsInside`
  style, with a separator boundary).
- **R4.8** Accept a downloaded file only when it passes the checks Octo already makes (extension
  allow-list, size, decodes as audio); do not execute, extract or open anything else. Archives
  from peers are not unpacked.
- **R4.9** Peer-supplied text is HTML-encoded wherever it is shown, and is treated as data in
  logs.
- **R4.10** slskd is reached only with its API, over the compose network. Octo does not forward
  user input into slskd's configuration (`SLSKD_REMOTE_CONFIGURATION` writes are limited to the
  share list Octo itself builds).
- **R4.11** What is shared with Soulseek is read-only and excludes Octo's working folders
  (`.octo-incoming`, `.octo-trash`, partial files). A change to sharing needs Security review.

**Other external input**

- **R4.12** Any URL a user or remote service supplies is fetched only after a scheme and host
  allow-list check (as `SpotifyLinkReader` does by building the host itself). No fetch of an
  arbitrary user URL from inside the compose network.
- **R4.13** Uploaded files and archives have a byte limit, an entry-count limit and an unpacked
  size limit (`ImportFileReader`: 64 MB, 200 entries, 256 MB). Copy those limits.
- **R4.14** Images from the network or from tags are untrusted input to the image library; see
  dependency hygiene, and cap dimensions and bytes before decoding.

### 5. File-system deletion safety

- **R5.1** Deletion of anything under the music root goes through the quarantine path
  (`LibraryActionQuarantine`): move to the trash folder with a manifest, never a direct delete,
  with a retention period before final removal.
- **R5.2** Before moving or deleting, resolve the full path and confirm it is inside the music
  root with a separator boundary, and that the file is a regular file and not a symlink or
  reparse point.
- **R5.3** Every bulk or scheduled delete has a **dry-run** mode that is the default for a new
  feature, reports what it would remove, and a hard cap on the count per run. Running it
  without dry-run on real data needs written human approval (`AGENTS.md`).
- **R5.4** A delete decision never rests on user-controlled text alone (a title, a typed path).
  It rests on an id Octo issued, resolved to a path by Octo.
- **R5.5** Cleanup jobs delete only inside one named folder Octo created (`.octo-incoming`,
  cache folders) and by age, and re-check the path boundary per entry.
- **R5.6** `Directory.Delete(..., recursive: true)` is allowed only on a folder Octo created
  and owns, never on a configured or user-supplied path.
- **R5.7** Each deletion writes a journal entry (who asked, what, when) that survives a restart,
  so a mistake can be traced and undone.
- **R5.8** Tests cover a path with `..`, an absolute path, a sibling folder with the same prefix
  (`/music2` next to `/music`), and a symlink.

### 6. Dependency hygiene

- **R6.1** A PR that adds a package, crate, pip package, container or external service lists it in
  the PR body with the reason and licence, and needs human approval if the approved plan does
  not already list it (`AGENTS.md`).
- **R6.2** Versions are pinned: NuGet `Version=` exact, `Cargo.lock` committed, pip with `==` (or a
  hash-locked file), container images by a version tag (digest where practical), and release
  downloads by version with a checksum. `latest` is not allowed for new work. (Baseline gaps:
  SEC-7.)
- **R6.3** Before a PR is opened the author runs the scans below and the result has no
  High/Critical finding that has a fix available:
  - `dotnet list package --vulnerable --include-transitive`
  - `cargo audit` in `sonic/`
  - `pip-audit` against the shim's requirements
- **R6.4** CI runs the same three scans on every PR and on a weekly schedule, and fails on High or
  Critical. (Not in place today; see SEC-7.)
- **R6.5** A vulnerable dependency with no fix is recorded on the issue with the exposure and the
  mitigation, and re-checked at each release. Accepting a Critical or High as "won't fix"
  needs human approval.
- **R6.6** Anything that parses untrusted data (images, audio tags, archives, HTML, JSON from
  peers) is on the "keep patched" list: image library, tag library, ffmpeg, yt-dlp.
- **R6.7** Containers run as a non-root user where the workload allows (as `octo-sonic` does)
  with read-only mounts for what they only read. A new container that needs root says why.

---

## Part 2. Baseline audit

### Scope and method

Repository at `origin/main` `aac1c7a`. Read: `octo/Middleware/*`, `octo/Services/Admin/*`,
`octo/Controllers/AdminController.cs` (sign-in parts), `ImportsController`,
`ImportsSubsonicController`, `octo/Services/Imports/*`, playlist mutation in
`SubSonicController.cs`, `ImportPlaylists`, `NavidromePlaylistApi`, the deletion paths in
`octo/Services/Library` and `octo/Services/Soulseek`, `PathHelper`, `yt-dlp-shim/app.py`, both
Dockerfiles, `docker-compose.yml`, `install.sh` and `.env.example`. This was a read-through, not
a penetration test; nothing was run against a live server. Not reviewed in depth: Last.fm,
Lidarr and ListenBrainz clients, the dashboard JavaScript (XSS), `UpdateController` and the
update helper, `sonic/src`.

### Dependency scans

Run 2026-10-10 in throwaway containers; no project files changed.

| Ecosystem | Command | Result |
|---|---|---|
| .NET | `dotnet list octo.sln package --vulnerable --include-transitive` (SDK 9.0 container, after `dotnet restore`) | **1 vulnerable package**: `SixLabors.ImageSharp` 3.1.12 (top-level in `octo`, transitive in `octo.Tests`). Advisories: 3 High (GHSA-j3p4-wp97-rph4, GHSA-j9gm-c75j-xc9q, GHSA-jjfr-hcj7-qf5w), 2 Moderate (GHSA-gwg2-r3hj-4w44, GHSA-wmxv-xphr-5c9g). No other package, direct or transitive, is flagged. See SEC-1. |
| Rust | `cargo audit -f Cargo.lock` in `sonic/` (rust:latest container, cargo-audit installed with `--locked`) | **No vulnerabilities and no warnings**: 125 crates scanned against 1296 RustSec advisories. |
| Python | `pip-audit -r` over the shim's installed packages (`flask`, `gunicorn`, `requests`, `ytmusicapi>=1.12,<2`; the shim has no `requirements.txt`, these come from `yt-dlp-shim/Dockerfile:47`) in a throwaway venv | **No known vulnerabilities.** Caveat: unpinned, so this audits today's resolved versions, not necessarily the versions in a published image. yt-dlp, deno and ffmpeg are not pip packages and are not covered by any scan (SEC-7). |

### Git history check

`gitleaks 8.30.1`, `detect --log-opts=--all`: 953 commits, 16.7 MB scanned, 8 hits, reported
redacted. All 8 reviewed and judged **false positives (placeholder or test data)**. No secret
value is reproduced here.

| Path | Commits | What it is |
|---|---|---|
| `scripts/test-install.sh` (line 62) | `76684ab3`, `b204e416` | A short placeholder `LASTFM_API_SECRET` in a test `.env` fixture (6 characters, not a real key shape). |
| `octo.Tests/LastFmScrobbleTests.cs` (line 550) | `1bfb9314`, `eccaaab1` | A sequential-hex constant used as a fake `ApiKey` in a test. |
| `docs/song-identity-cases.json` (line 224) | `d5e86462`, `1829c085` | A song title containing `$` characters, matched by the generic-key rule. |

Also checked: `git log --all --name-only` shows no committed `.env`, `*.pem`, `*.key`, or
`spotify-accounts.json`, `octo-config` or session file; `/.env` is in `.gitignore`
(line 71). Conclusion: no committed secrets found. gitleaks looks at the diff text only; a
secret in an earlier force-pushed or deleted branch that was never fetched would not show up.

### Findings

No Critical finding. Totals: **Critical 0, High 2, Medium 5, Low 6.**

| Id | Severity | Finding | Where |
|---|---|---|---|
| SEC-1 | High | Image library with three High advisories processes untrusted images | `octo/octo.csproj:45` |
| SEC-2 | High | slskd web UI and API published to all interfaces, default login `slskd`/`slskd` unless installer ran | `docker-compose.yml:396-397`, `:68-69`, `:414-415`, `:402`; `.env.example:120-121` |
| SEC-3 | Medium | Octo's port, with the dashboard sign-in, is published on every interface by default | `docker-compose.yml:6-7` |
| SEC-4 | Medium | Spotify access and refresh tokens stored in plain text; file readable between write and chmod | `octo/Services/Imports/SpotifyAccountStore.cs:8-9, 92-96` |
| SEC-5 | Medium | Admin recovery code written to the application log | `octo/Program.cs:650` |
| SEC-6 | Medium | Service passwords and API keys kept in plain text in `settings.json`, written without restricting its mode | `octo/Services/Admin/SettingsFileWriter.cs:156-164` |
| SEC-7 | Medium | Unpinned downloads and images, runtime self-update, no dependency scan in CI | `yt-dlp-shim/Dockerfile:3-4, 15-16, 36-37, 47`; `yt-dlp-shim/app.py:668`; `docker-compose.yml:393, 453`; `.github/workflows/ci.yml` |
| SEC-8 | Low | Shim does not validate the video id and passes `dest` unescaped into a yt-dlp output template | `yt-dlp-shim/app.py:806-807, 931-947, 563-569` |
| SEC-9 | Low | Dashboard sessions last 90 days sliding; sessions file written without a mode; sign-in throttle is per address in memory | `octo/Services/Admin/BrowseSessionStore.cs:27, 156-157`; `octo/Services/Admin/SignInThrottle.cs:16, 52` |
| SEC-10 | Low | Directory browser lists any folder of the container for a signed-in admin | `octo/Services/Admin/DirectoryBrowser.cs:73-80`; `octo/Controllers/AdminController.cs:556` |
| SEC-11 | Low | Octo and shim containers run as root | `Dockerfile:17` (no `USER`), `yt-dlp-shim/Dockerfile` (no `USER`) |
| SEC-12 | Low | `ADMIN_SIGN_IN=off` removes all admin protection | `octo/Middleware/AdminSignInGate.cs:32-37`; `octo/Program.cs:643` |
| SEC-13 | Low | Subsonic credentials travel in the query string and CORS allows every origin on the Subsonic API | `octo/Program.cs:608-616`; protocol-imposed |

#### SEC-1 (High) SixLabors.ImageSharp 3.1.12 has known advisories

`octo/octo.csproj:45`. Octo decodes cover images from the network and from file tags
(`octo/Services/CoverArt/*`), which is untrusted input, exactly what the advisories concern
(3 High, 2 Moderate on the pinned version).
**Fix:** move `SixLabors.ImageSharp` (and check `ImageSharp.Drawing` 2.1.7 against it) to the
release the advisories name as fixed, run the CoverGolden tests (`octo.Tests/CoverGolden`), and
cap image dimensions and bytes before decoding (R4.14). Add the .NET scan to CI (SEC-7). If
the fixed release needs a licence change or a new major version, that is a human decision
(`AGENTS.md`, new dependency rule).

#### SEC-2 (High) slskd exposed with default credentials and remote configuration on

`docker-compose.yml:396-397` publishes `5030` (slskd's web UI and API) and `50300` (peer port) on
all interfaces. The login defaults to `slskd`/`slskd` when `.env` sets nothing
(`docker-compose.yml:68-69, 414-415`; `.env.example:120-121` ships exactly that), and
`SLSKD_REMOTE_CONFIGURATION` is `true` (`:402`) so slskd's administrator can rewrite its own
configuration, while slskd has the library mounted read-write (`:439`). `install.sh:338-341`
generates a random password, so installs made by the installer are not affected; a manual
`cp .env.example .env` install is. The compose comment says "keep it on your network, never
forward it", which is a convention, not a control.
**Fix:** (a) bind `5030` to `127.0.0.1` by default with an `.env` setting to widen it
(R3.8); Octo reaches slskd over the compose network and does not need the host port;
(b) remove the `slskd` default from `.env.example` and compose, so an empty value makes the
stack refuse to start with a clear message, or have Octo/installer generate one;
(c) keep `50300` published (it must be reachable by peers) but document it as the only intended
internet-facing port.

#### SEC-3 (Medium) Dashboard reachable wherever the main port is

`docker-compose.yml:7` publishes `5274` on all interfaces, and by default (`ADMIN_PORT` unset)
the dashboard and `/api/admin` answer on the same port as the Subsonic API, so forwarding
`5274` to the internet for music apps also exposes the admin sign-in. The sign-in itself is
sound (Navidrome-vouched, one-use recovery code, throttled, `HttpOnly`/`SameSite=Strict`
cookie, CSRF guard, no CORS on admin) and `ADMIN_PORT` already solves the exposure, but it is
opt-in and commented out.
**Fix:** make the split the default for new installs (installer sets `ADMIN_PORT` and the
`127.0.0.1` mapping), or at least have the README say that `5274` must not be forwarded without
setting `ADMIN_PORT`. Navidrome's own `4533` (`:463`) is also published on all interfaces when
the starter profile is on; the same default-to-LAN advice applies.

#### SEC-4 (Medium) Spotify tokens in plain text, with a permission window

`SpotifyAccountStore` writes access and refresh tokens as JSON into
`spotify-accounts.json` beside `settings.json` (`Program.cs:345`). Good: a separate file keyed
by Navidrome user, written through a temporary file, mode `0600` on Unix, and the dashboard
never shows it. Gaps: tokens are not encrypted; the temp file is created with the default
umask and `SetUnixFileMode` runs only after `WriteAllText` (lines 92-95), so the content is
briefly readable and, if the chmod throws, a readable `.tmp` is left behind (the exception is
swallowed at line 98); the refresh token lives six months. Anyone with the `octo-config` mount
has every user's Spotify access. The container runs as root (SEC-11), which makes `0600`
protect against other host users only.
**Fix:** create the temp file with the restrictive mode from the start (R1.4); delete the temp
file on failure; consider encrypting the token fields with a key from a separate secret
(`Data Protection` or an env key) so a copied config folder does not carry live tokens;
remove tokens when a Navidrome user is deleted. Also log a warning if the chmod is
refused, so the operator knows.

#### SEC-5 (Medium) Recovery code is logged

When no Navidrome URL is set, `Program.cs:650` logs the live recovery code in clear text
("sign in to the dashboard with the recovery code {Code}"). The code gives a one-hour admin
session that can read and write every setting, including stored keys. Docker logs are often
shipped to other systems or shown in support threads.
**Fix:** log only the file path (the code is already in that file, readable by whoever owns the
host), or print the code to the console once at first start and never again; never log it
through the logging pipeline (R1.2).

#### SEC-6 (Medium) Other secrets in `settings.json` in plain text

Service passwords and API keys (Navidrome admin, slskd, Lidarr, Last.fm, AcoustID) are kept in
`settings.json` when saved from the dashboard and in the container environment. Masking on
display is done (`AdminController.cs:807-891`), which is right. But `SettingsFileWriter.Write`
(`SettingsFileWriter.cs:156-164`) writes the file with the default mode and no restriction,
and `chmod` is applied to only two files in the whole code base
(`AdminRecoveryCode.cs:77`, `SpotifyAccountStore.cs:95`).
**Fix:** write `settings.json` with mode `0600` (R1.4); keep secrets in a separate file from
non-secret settings so exports and screenshots of the settings file are safe; document that
the `octo-config` folder is as sensitive as `.env`.

#### SEC-7 (Medium) Unpinned dependencies and no scanning in CI

- `yt-dlp-shim/Dockerfile:15-16` and `:36-37` download yt-dlp and deno from "latest" release
  URLs over HTTPS with no checksum or signature check; `:47` installs `flask`, `gunicorn`,
  `requests` unpinned; the base image is a floating `python:3.12-slim`.
- `yt-dlp-shim/app.py:668` (`_maybe_self_update`) fetches a new yt-dlp binary at run time into
  the `ytdlp-bin` volume. That is a deliberate availability choice (YouTube changes often), but
  it is code from the network running in a container that can write to the music folder.
- `docker-compose.yml:393` and `:453` run `slskd/slskd:latest` and `deluan/navidrome:latest`.
- `.github/workflows/ci.yml` has no vulnerability scan for any ecosystem; no dependency update
  tool is configured.
**Fix:** pin yt-dlp and deno versions in the build and verify the published SHA-256; pin the
pip packages (hash-locked file); keep yt-dlp's self-update but verify the download against the
published checksum and restrict the shim to read-write only on its own staging folder;
pin sidecar images to version tags (digest preferred) and let the update helper bump them;
add `dotnet list package --vulnerable`, `cargo audit` and `pip-audit` jobs to CI (R6.4) and a
weekly scheduled run. Changes to `.github/workflows/*` are a shared hotspot and need a claim.

#### SEC-8 (Low) Shim input validation is thin

The shim is only reachable on the compose network (no `ports:` for it, checked), which is why
this is Low. Still: `/stream` and `/download` take `id` and use it in a URL and as a cache key
without checking its format (`app.py:807, 931`); it is embedded in a fixed
`https://www.youtube.com/watch?v=` URL so it cannot become a yt-dlp option or change the host,
but extra query text can be smuggled in. `dest` is confined to the download root with a
separator boundary (`app.py:939-941`, good) but is then placed in yt-dlp's `-o` template
(`:949`), where `%(...)s` fields in the name would be expanded. Search text is safely behind
`ytsearch1:`/`ytsearch5:`. The pre-download clean-up in `_sweep_partials` removes
`<dest>.mp3` if the download fails, which is only safe because Octo picks `dest`.
**Fix:** validate `id` as 11 characters of `[A-Za-z0-9_-]` and answer 400 otherwise; escape `%`
in `dest`; add the checks to the shim's tests (R4.2, R4.3).

#### SEC-9 (Low) Session lifetime and sign-in throttle

Dashboard sessions slide for 90 days (`BrowseSessionStore.cs:27`); only a hash of the token is
stored (good), but the sessions file is written without a restricted mode (`:156-157`). The
sign-in throttle (10 failures in 15 minutes) is keyed by remote address in memory
(`SignInThrottle.cs:16, 52`); behind a reverse proxy where only `X-Forwarded-Proto` is honoured
(`Program.cs:35-42`) every client shares one address, as the class comment says: this fails
closed, so it is a nuisance rather than a bypass.
**Fix:** shorten the sliding lifetime or add an absolute maximum (for example 30 days), list and
revoke sessions on the dashboard (sign-out "everywhere" exists at `AdminController.cs:644`),
and set `0600` on the sessions file.

#### SEC-10 (Low) Directory browser is not confined

`DirectoryBrowser.Browse` canonicalises the path but then lists any directory the container can
read, starting at `/` (`DirectoryBrowser.cs:63-80`). It is for choosing the music folder and
is behind the admin sign-in. It shows folder names and counts only, but names can be sensitive
and it is more reach than the job needs.
**Fix:** confine it to the mounted volumes (`/music`, and the configured download path) and
refuse the rest; or show only the folders that are bind mounts.

#### SEC-11 (Low) Containers run as root

The Octo image (`Dockerfile`, final stage at line 17) and the shim image set no `USER`, so both
run as root with the music folder mounted read-write. `octo-sonic` already drops to
`65534:65534` with a read-only root file system and a read-only music mount
(`docker-compose.yml:374-382`); that is the model to copy.
**Fix:** run Octo and the shim as an unprivileged user with the matching `PUID/PGID` or a
documented `user:` setting, noting the bind-mount ownership change this implies for existing
installs (a migration step for the update helper).

#### SEC-12 (Low) Sign-in can be switched off

`ADMIN_SIGN_IN=off` (`AdminSignInGate.cs:32-37`) turns the whole admin gate off, and
`Program.cs:643` logs a warning. It is an explicit choice for people who sign in elsewhere,
so Low, but with it on and `5274` forwarded the dashboard is open to the internet.
**Fix:** when the sign-in is off, refuse requests whose remote address is not a private or
loopback address, unless a second setting says otherwise.

#### SEC-13 (Low) Subsonic credentials in the URL; wide-open CORS on the Subsonic API

The Subsonic protocol sends `u`, `t`, `s` (or `p`, or `apiKey`) in the query string, so they can
end up in proxy and access logs; CORS allows every origin (`Program.cs:608-616`) because web
players need it. The admin API is protected from this (`AdminRequestGuard` strips CORS headers
and requires `X-Octo-Admin`). This is protocol-imposed, so not a defect to fix alone.
**Fix:** none needed in Octo beyond keeping query strings out of Octo's own logs (not audited here: confirm
ASP.NET request logging and the Subsonic relay never log the query) and telling users to
prefer HTTPS when exposing Octo.

### Reviewed and found sound

- **Admin sign-in** (`AdminSignInGate`, `AdminRequestGuard`, `AdminCookie`, `AdminRecoveryCode`):
  one gate in front of everything under `/api/admin`; Navidrome vouches for the admin, with an
  hourly role re-check; credentials only in the POST body; one-use recovery code compared in
  constant time and replaced after use; throttle; recovery sessions last one hour, are never
  written to disk and cannot reach library-file endpoints; security headers on admin pages.
- **Cross-user access in Imports:** every `ImportService` method takes the user and
  `ImportStore` filters by owner (`ImportStore.cs:34-60`); a list id from another user is
  "no such list". Dashboard routes take the user from the session
  (`ImportsController.cs:SignedIn`), Subsonic routes from credentials Navidrome accepted
  (`ImportsSubsonicController.cs:37-51`), and an API key alone is refused for imports. The
  Spotify callback uses an unguessable single-use `state`, a 15-minute expiry and PKCE
  (`SpotifyAuth.cs:28-51`), and refuses a finish started by someone else
  (`ImportService.cs:229-232`). One thing this audit could not confirm: the Subsonic path
  trusts the `u` parameter after Navidrome accepts the credentials; this holds because
  Navidrome refuses `u` together with `apiKey` (`RequestIdentity.cs` doc comment), which is
  Navidrome behaviour and worth a test (R2.7).
- **Playlists:** create/update/delete are relayed to Navidrome with the caller's own
  credentials, so Navidrome's ownership rules apply (`SubSonicController.cs:732-748`); Octo's
  generated playlists are read-only. Imports make a playlist as the admin and hand it to the
  user (`ImportPlaylists.cs`), which is documented and logs when the hand-over fails.
- **File deletion:** library removals go to a quarantine with a manifest, resolve the path
  inside the music root with a separator boundary, refuse links
  (`LibraryActionQuarantine.cs:154-182`, `NavidromeSongPathResolver.cs:328-335`), support dry
  run; Soulseek clean-up is limited to Octo's own incoming folder and by age
  (`SoulseekDownloadService.cs:380-395, 525-540`). Names from peers are sanitised, and `..`
  becomes `Unknown` (`PathHelper.cs:132-190`).
- **Upload handling:** the import file reader caps bytes, entries and unpacked size
  (`ImportFileReader.cs:25-34`); `SpotifyLinkReader` builds the host itself.
- **Ports:** only `5274` (Octo), `5030` and `50300` (slskd) and, with the starter profile,
  `4533` (Navidrome) are published. The shim and `octo-sonic` publish nothing. The dashboard
  port is opt-in and bound to `127.0.0.1` in the commented example.

### Proposed follow-up issues (not yet created; awaiting human approval)

Critical/High findings, for the human to approve before any issue is filed:

1. **Upgrade SixLabors.ImageSharp past the High advisories** (SEC-1): move to the fixed
   release, rerun cover tests, cap image size before decoding. Owner: Backend.
2. **Stop publishing slskd with default credentials** (SEC-2): bind `5030` to `127.0.0.1` by
   default and refuse to start with `slskd`/`slskd`. Owner: Backend.

Suggested for the same approval round, Medium: restrict secret-file permissions from creation
(SEC-4, SEC-6, SEC-9); stop logging the recovery code (SEC-5); dependency pinning and CI scan
jobs (SEC-7); default the dashboard to its own port (SEC-3).
