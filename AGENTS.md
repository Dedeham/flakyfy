# AGENTS.md

Guide for AI agents (and humans) working on Flakyfy, a fork of [winters27/octo](https://github.com/winters27/octo).
Flakyfy is a Subsonic-API proxy in front of Navidrome that finds and downloads missing songs
(Soulseek via slskd, Lidarr, YouTube via yt-dlp), with a web dashboard, imports, radio and covers.
The fork started clean from upstream `d2bcf723` (release 2026.10.15). Build on top; keep upstream
merges easy (`git remote` has `upstream`).

## Required tooling: ponytail and ecc

Both are Claude Code plugins, declared in `.claude/settings.json`. Accept the install prompt on first
launch (or `/plugin marketplace add DietrichGebert/ponytail`, `/plugin marketplace add affaan-m/ECC`,
then `/plugin install ponytail@ponytail` and `/plugin install ecc@ecc`).

- **ponytail**: before writing code, climb its ladder: does it need to exist, is it already in this
  codebase, std library, platform feature, installed dependency, one line, only then the minimum.
  Never skip validation, error handling, security or accessibility. Run `/ponytail-review` before
  opening a PR and `/ponytail-audit` for repo-wide checks.
- **Always, for every change and every agent**: ponytail governs what gets written and ecc how it
  gets done. Test, validate and ensure the result before calling it done; say plainly what could
  not be verified.
- **ecc**: follow plan, test, implement, review, verify. Use its planner/reviewer/security agents and
  AgentShield scan when touching agent, hook or MCP config.
- Other agent tools (Codex, Cursor, ...) should read the same rules from this file.

## Stack at a glance

| Part | Path | Tech | Notes |
|---|---|---|---|
| Server (shipped) | `crates/` | Rust 1.99 (`rust-toolchain.toml`), edition 2024, axum 0.8, tokio, reqwest (rustls), lofty, tiny-skia + cosmic-text | Workspace: `flakyfy` (bin, app wiring in `app.rs`, ~4000 lines), `flakyfy-core` (settings, Soulseek, Lidarr, Last.fm...), `flakyfy-subsonic`, `flakyfy-media`. `unsafe` is forbidden. |
| Audio similarity | `sonic/` | Rust, bliss-audio + FFmpeg | Own `Cargo.lock`, excluded from the workspace. Reads each file in a child process because FFmpeg can crash. |
| yt-dlp shim | `yt-dlp-shim/` | Python 3.12, Flask, ytmusicapi | Downloads yt-dlp and deno at image build. |
| Dashboard | `app/wwwroot/` | Vanilla JS, no build step | `admin/` and `family/`; served by both servers. Icon sprite: `scripts/admin-icons/make_sprite.py`. |
| Deploy | `Dockerfile`, `docker-compose.yml`, `install.sh`, `scripts/updater/` | Docker, systemd | Images `ghcr.io/dedeham/flakyfy`, `-yt-dlp-shim`, `-sonic`. |

State is JSON files under `/app/config` (no database); music is in `/music`.
Config order: `appsettings.json`, then env vars, then `/app/config/settings.json` (written by the
dashboard, overrides everything). `.env.example` uses flat `SCREAMING_SNAKE` names that
`docker-compose.yml` maps to settings (the `Section__Key` form also works).
Auth: Subsonic clients use Navidrome credentials; the dashboard signs in with a Navidrome admin
account; the Rust-only Family feature seals member passwords with ChaCha20-Poly1305 under `family.key`.

## Commands

**CI runs on GitHub, never locally.** Building the Rust workspace (`cargo build`, `check`, `clippy`,
`test`) crashes the owner's machine, even with few jobs. Locally run only the light checks:
`cargo fmt --all --check`, `node --check app/wwwroot/admin/*.js`, `shellcheck`. Then push the branch,
open the PR and read the results there: `gh pr checks <n>` (add `--repo winters27/octo` for an Flakyfy PR)
and `gh run view --log-failed`; fix and push again. For an Flakyfy PR, read Flakyfy's own CI on that PR
instead of dispatching `ci.yml` on the fork first. A PR with merge conflicts gets no CI, so rebase first.

What CI runs (`.github/workflows/ci.yml`), for reference and for reading failures:

```bash
# Rust server (root)
cargo fmt --all --check
cargo clippy --workspace --tests -- -D warnings
cargo test --workspace
cargo deny check bans licenses sources        # needs cargo-deny

# sonic (needs FFmpeg dev libs, clang, pkg-config)
cd sonic && cargo clippy --all-targets -- -D warnings && cargo test

# shim
pip install -r yt-dlp-shim/requirements-dev.txt && pytest yt-dlp-shim/tests -q


# scripts
shellcheck -S warning install.sh scripts/*.sh scripts/updater/*.sh
bash scripts/updater/test-updater.sh && bash scripts/test-install.sh
```

Tests that need ffmpeg, chromaprint or fonts skip themselves when missing; CI installs
`ffmpeg libchromaprint-tools fonts-noto-cjk fonts-symbola fonts-dejavu-core`.
`rustfmt.toml`: max width 110. Never share a `CARGO_TARGET_DIR` between worktrees.

## Conventions

- **Commits**: `type(scope): what is now true, in plain words`, lowercase, no trailing period.
  Types seen: fix, feat, parity, docs, test, perf, style, chore, build, release. Scopes are domain
  nouns (`radio`, `health`, `tags`, `covers`). Example: `fix(radio): a history reset stays reset`.
- **Branches**: `type/kebab-description` (`feat/...`, `fix/...`, `docs/...`).
- **Rust**: mirrors the C# shapes on purpose; keep behavior-compatible until parity is retired.
  Intentional C#/Rust differences go in `docs/rust-migration/known-diffs.md`.
- **C# layout** (legacy): one folder per domain under `flakyfy/Services/<Domain>`, one type per file,
  `IFoo`/`Foo` pairs, `*Worker` for background services, namespace `Flakyfy.Services.X`.
- **Tests**: C# tests are `<Subject>Tests.cs`, named `Method_Condition_Result`. Shared data-driven
  cases live in `docs/*-cases.json` and `docs/fuzzy-golden.json`; Rust golden tests read
  `parity/recordings/csharp/`. Do not weaken or delete a test to get green.
- **Docs**: `.gitignore` ignores every `*.md` except `README.md`, `AGENTS.md`, `docs/family.md`,
  `docs/rust-migration/known-diffs.md` and `scripts/admin-icons/SOURCES.md`. To commit
  another markdown file, add an exception there. New settings go in `README.md`, `.env.example`
  and `docker-compose.yml` with their default.

## Workflow: branches, merges, upstream

Remotes: `origin` = this fork (`Dedeham/flakyfy`), `upstream` = `winters27/octo`.
Never commit on `main`. Every feature, fix or doc change starts on its own branch.

1. **Sync first**: `git fetch upstream origin`.
2. **Pick the base**, because it decides whether the work can go back to Flakyfy:
   - **Upstreamable** (a bug fix, a feature Flakyfy would want): `git switch -c type/kebab-description upstream/main`.
     The branch then holds only your commits on top of Flakyfy's history, so the PR to
     `winters27/octo` is clean.
   - **Fork-only** (Flakyfy branding, fork CI, `AGENTS.md`, fork-specific settings):
     `git switch -c type/kebab-description main`. These never go upstream.
   - Do not mix both kinds in one branch. Split them.
3. **Name** it `type/kebab-description` with the same types as commits (`feat/`, `fix/`, `docs/`,
   `chore/`, `perf/`, `test/`). Short and specific: `feat/album-search`, not `feat/stuff`.
4. **Commit** small, one finished unit per commit, in the format under Conventions.
5. **Before pushing**: run the light local checks above (never a cargo build), then `/ponytail-review`; CI runs on GitHub after the push. Rebase onto the latest base
   (`git rebase upstream/main` or `git rebase main`) so the history stays linear.
6. **Push and open a PR** from the branch to `main` of this fork. An upstreamable branch is then also
   offered to Flakyfy: `gh pr create --repo winters27/octo --base main --head Dedeham:<branch>`.
   Wait for the owner's OK before opening the upstream PR (it is outward-facing).
7. **Merge into `main`** only through the PR, squash or rebase, never a merge commit that drags in
   unrelated work. Delete the branch afterwards.

Pulling Flakyfy's changes into the fork:

- `git fetch upstream`, then on a branch `chore/sync-upstream-YYYY-MM-DD` run `git merge upstream/main`
  (a merge, not a rebase, so `main` is never rewritten), open a PR and let GitHub CI run.
- Enable `git config rerere.enabled true` so a conflict is resolved once and remembered.
- Keep upstream merges conflict-free: change files Flakyfy owns as little as possible, put new code in new
  files/modules, and keep fork-only config (branding, workflows) in separate files. Do not reformat or
  rename upstream code (see "Agent rules").
- A feature that was merged upstream under different commits resolves cleanly on the next sync if the
  fork's copy is dropped in favour of Flakyfy's; take Flakyfy's version on conflict and re-apply only the fork delta.
- Never force-push `main`. Force-push (`--force-with-lease`) only your own feature branch.

## Octo app compatibility (permanent)

Flakyfy is only the server, shipped as Docker images. Its apps are Octo's (`winters27/octo-player`,
released at `winters27/octo`) and cannot be changed from here. They must keep working in every
release, so these names stay Octo's forever, whatever else is called Flakyfy:

- Extensions and their envelope: `octo*` names, `"type": "octo"`.
- Fields and parameters: `octoList`, `octoLists`, `octoStations`, `octoGroup`, `octoNotice`,
  `octoCoverTitle`, `octoSuggestedBy`, `octoPersonal`, `octoCatalog`, `octoFamily*`, `octoMix`,
  `octoSongToRemove`.
- Identity: client `c=Octo`, the `Octo` User-Agent prefix, `octo` in `/api/admin/status`,
  `{"octo":"family"}` from `/family/hello`.
- Headers and cookies: `X-Octo-*`, `octo_browse`, `octo_family`.
- Links: `octo://`, Android package `app.winters.octo`.
- On disk, so an Octo install needs no data moved: `.octo-*` folders and marks, slskd's `Octo: sharing`
  block, `places.bin` header `OCTOPLC1`, `ReviewSweepOctoDownloads`, `OctoDownloads`.

Product text, the logo, images, binaries, service names and `FLAKYFY_*` settings are Flakyfy. Never
rename an Octo name above, not in a cleanup and not in an upstream sync (take Octo's name on
conflict). `scripts/test-octo-contract.sh` and the Rust route tests guard this and must stay. When a
new app release adds a name, add it there. Compare with `octo-player`, the source of truth, when in
doubt.

## Hard rules (from upstream's README)

- An empty allowlist means nobody, never everybody.
- Five stars means Keep and is never destructive; Review actions stay behind a dry-run flag.
- Secrets never come back out of the admin API; logs redact `t s p apiKey token api_key client`.
- Flakyfy never gets Docker access. The updater works through a file drop (`update/request` in the
  config folder) and a root systemd helper (`scripts/updater/`).
- Admin writes carry the `X-Octo-Admin: 1` header; keep the guard when adding endpoints.
- Never set `CPUS` above the machine's core count.

## Cutting a release

Releases read like Octo's. Title `Flakyfy YYYY.MM.DD`. Notes are short themed sections that say what
changed for the listener, in plain words, with `(#PR)` and `Thanks to @user`, then `Fixes`, `Apps`
and `Updating`:

```
## Updating

Press About → Update now as usual. Nothing to change on your side.

Without the update helper, from the Flakyfy folder:

    git fetch --tags && git checkout --detach YYYY.MM.DD
    docker compose pull flakyfy yt-dlp-shim flakyfy-sonic
    docker compose up -d
```

`Apps` names the Octo app releases that come out alongside, and the links block is added by
`release-links.yml`. Steps: on a branch, a `release: YYYY.MM.DD` commit bumps `VERSION`, every
`FLAKYFY_IMAGE_TAG` default in `docker-compose.yml` and the README's app version links when the apps
moved; merge it; `gh release create <tag> --draft`; the owner reviews the notes; push the tag.
`docker.yml` publishes the images, then the draft. A new ghcr.io package starts private and must be
made public once by the owner.

## Release and CI: what is dangerous

- Tags `v*` and `YYYY.MM.DD[.N]` (and every push to `main`) trigger `docker.yml`, which builds
  multi-arch images and pushes them to ghcr.io; a dated tag also publishes the GitHub release.
  `VERSION` must equal the tag and every `FLAKYFY_IMAGE_TAG` default in `docker-compose.yml`.
  Pushing a tag or merging to `main` publishes real images, so it needs the owner's OK each time.
- Version quirks: `VERSION` is dated (`2026.10.15`); the Cargo workspace is `2026.10.5` (no leading
  zero allowed); the displayed version comes from `FLAKYFY_VERSION`, then `VERSION`.
- `scripts/updater/` and `install.sh` change a real host (root, git, docker). Never run them
  against a real server; the updater tests use a throwaway origin with Docker faked.
- `docker-compose.yml` uses `:latest` for slskd and navidrome. The sonic CI job uses stable Rust,
  not the pinned toolchain. `deny.toml` mentions a `rust.yml` that does not exist.
- Root `package-lock.json` is an empty stub; there is no JS build.

## Agent rules

- No `Co-Authored-By` trailers or "Generated with" lines naming an AI in commits, PRs or files.
- No personal data (names, emails, listening history) in commits, PRs, fixtures or issues; this
  repository is public.
- Never commit secrets or `.env`; add new settings to `.env.example`.
- Do not push to `main`, push tags, publish images, or run the updater/installer against a real
  server without explicit approval from the owner. Work on a branch and open a PR.
- For any new feature or fix, create the branch first (see "Workflow: branches, merges, upstream"),
  asking whether it is upstreamable or fork-only if that is unclear. Never work on `main`.
- Do not reformat or "clean up" code outside the task's scope.
- Use sonnet for subagents.
