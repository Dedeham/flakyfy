# Testing

Flakyfy has three layers of tests. Each answers a different question.

| Layer | Question | Where | Run it |
|---|---|---|---|
| Unit tests | Does this piece of code do its job? | `octo.Tests/` (.NET), `yt-dlp-shim/tests/` (pytest), `sonic/` (cargo) | `dotnet test`, `pytest yt-dlp-shim/tests -q`, `cargo test` in `sonic/` |
| Shell tests | Do the installer and updater keep their promises? | `scripts/test-install.sh`, `scripts/updater/test-updater.sh` | `bash scripts/test-install.sh` |
| Integration harness | Does Octo behave across services and across users, against a real Navidrome? | `tests/integration/` | `tests/integration/run.sh` |

`.github/workflows/ci.yml` runs the first two. `.github/workflows/integration.yml` runs the
harness on every pull request to `main`, as its own workflow so it cannot break the others.

## The integration harness

One command starts everything, seeds it, tests it and tears it down:

```
tests/integration/run.sh          # run, then remove containers and volumes
tests/integration/run.sh --keep   # leave the stack running to poke at
tests/integration/run.sh up       # start and seed only
tests/integration/run.sh down     # tear down
```

It needs Docker with Compose and Python 3 (standard library only, no packages). The first run
builds the Octo image, which takes a few minutes.

### The stack

`tests/integration/docker-compose.yml` is separate from the root `docker-compose.yml` and starts
two containers under the project name `fla12-it` (set `HARNESS_PROJECT` to change it):

- **Navidrome** (`deluan/navidrome`, the image the main stack already uses), on `127.0.0.1:14533`.
  It is the source of truth for users, plays, favorites and playlists
  (`docs/architecture.md`, section 4.2).
- **Octo**, built from the repository's `Dockerfile`, on `127.0.0.1:15274`, pointed at that Navidrome.

External sources are stubbed by being switched off or pointed at a host that does not resolve:
the yt-dlp shim, Soulseek, the Sounds alike service, YouTube Music and ListenBrainz radio,
Last.fm, Lidarr, cover and release lookups, and the update check. A test therefore never touches
the internet or a real account. If a module needs a working stand-in for one of them, add it to
the harness compose file as part of that module.

### Seed data

Built by `tests/integration/seed/`:

- `generate_library.py` writes 12 tagged WAV files (3 artists, 4 albums) of generated sine tones
  and silence. No recorded audio, so nothing is copyrighted. They are produced on every run and
  are not committed.
- `seed.py` creates an admin and three users (`alice`, `bob`, `carol`, with throwaway passwords
  that exist only inside the disposable stack), scans the library, then gives each user
  their own private playlists, favorites and play history. The users overlap on some songs on
  purpose, so a leak between users cannot hide behind lists that happen to differ.

### Tests

`tests/integration/test_*.py` are `unittest` files that talk to the stack over HTTP with
`harness.py` (a small Subsonic client). The first is `test_playlist_isolation.py`: each user sees
only their own playlists through Octo and directly on Navidrome, a playlist made through Octo
stays private to its owner, and deleting one user's playlist leaves the others alone.

## The rule for Wave 2 and later modules

**Every Wave 2 module adds its own validation workflow to this harness.** A module is not done
until its pull request includes:

1. Seed data for what it needs, added to `tests/integration/seed/` (extend, do not change what
   other tests rely on).
2. At least one `tests/integration/test_<module>.py` that runs against the seeded stack.
3. For anything stored per user, a check that one user cannot see or change another user's data.
4. Any stub or service it needs, in `tests/integration/docker-compose.yml`, never in the root
   `docker-compose.yml`.

The harness workflow already runs every `test_*.py` file, so no CI change is needed for a new
test file. The module's unit tests (`octo.Tests/<Module>/`, see `docs/modules.md`) still apply
on top of this.
