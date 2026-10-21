# AGENTS.md

Guide for AI agents (and humans) working on Flakyfy, a fork of [winters27/octo](https://github.com/winters27/octo).
Octo is a Subsonic-API proxy in front of Navidrome that finds and downloads missing songs
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
- **ecc**: follow plan, test, implement, review, verify. Use its planner/reviewer/security agents and
  AgentShield scan when touching agent, hook or MCP config.
- Other agent tools (Codex, Cursor, ...) should read the same rules from this file.

## Stack at a glance

| Part | Path | Tech | Notes |
|---|---|---|---|
| Server (shipped) | `crates/` | Rust 1.99 (`rust-toolchain.toml`), edition 2024, axum 0.8, tokio, reqwest (rustls), lofty, tiny-skia + cosmic-text | Workspace: `octo` (bin, app wiring in `app.rs`, ~4000 lines), `octo-core` (settings, Soulseek, Lidarr, Last.fm...), `octo-subsonic`, `octo-media`. `unsafe` is forbidden. |
| Server (legacy) | `octo/`, `octo.Tests/`, `octo.sln` | C# .NET 9 ASP.NET | Frozen at 2026.10.05, kept only until the Rust port is final. Built only by `Dockerfile.csharp`; never published. |
| Audio similarity | `sonic/` | Rust, bliss-audio + FFmpeg | Own `Cargo.lock`, excluded from the workspace. Reads each file in a child process because FFmpeg can crash. |
| yt-dlp shim | `yt-dlp-shim/` | Python 3.12, Flask, ytmusicapi | Downloads yt-dlp and deno at image build. |
| Parity harness | `parity/` | Python 3.10+ stdlib, shell | Black-box diff of the Rust build against the C# image. |
| Dashboard | `octo/wwwroot/` | Vanilla JS, no build step | `admin/` and `family/`; served by both servers. Icon sprite: `scripts/admin-icons/make_sprite.py`. |
| Deploy | `Dockerfile`, `docker-compose.yml`, `install.sh`, `scripts/updater/` | Docker, systemd | Images `ghcr.io/winters27/octo`, `-yt-dlp-shim`, `-sonic`. |

State is JSON files under `/app/config` (no database); music is in `/music`.
Config order: `appsettings.json`, then env vars, then `/app/config/settings.json` (written by the
dashboard, overrides everything). `.env.example` uses flat `SCREAMING_SNAKE` names that
`docker-compose.yml` maps to settings (the `Section__Key` form also works).
Auth: Subsonic clients use Navidrome credentials; the dashboard signs in with a Navidrome admin
account; the Rust-only Family feature seals member passwords with ChaCha20-Poly1305 under `family.key`.

## Commands

**The full CI runs on GitHub only.** Do not run the whole suite locally: building the `octo` tests
(its ~4000-line `app.rs`) exhausts memory on a dev machine and is OOM-killed (exit 137). Push the branch,
open the PR and let `.github/workflows/ci.yml` run these; read the result with `gh pr checks` and
`gh run view --log-failed`, and fix from there. Locally run only `cargo fmt --all --check`, cheap targeted
tests (`cargo test -p octo-core <name>`, `-p octo-media <name>`, shellcheck, pytest) and, to catch most
mistakes before a push, a compile-only pass that fits in memory:
`CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo clippy --workspace --tests -- -D warnings`
(about 5 minutes). Never build or run `-p octo` tests locally. What CI runs, for reference:

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

# legacy C#
dotnet build --configuration Release && dotnet test --configuration Release

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
- **C# layout** (legacy): one folder per domain under `octo/Services/<Domain>`, one type per file,
  `IFoo`/`Foo` pairs, `*Worker` for background services, namespace `Octo.Services.X`.
- **Tests**: C# tests are `<Subject>Tests.cs`, named `Method_Condition_Result`. Shared data-driven
  cases live in `docs/*-cases.json` and `docs/fuzzy-golden.json`; Rust golden tests read
  `parity/recordings/csharp/`. Do not weaken or delete a test to get green.
- **Docs**: `.gitignore` ignores every `*.md` except `README.md`, `docs/family.md`,
  `docs/rust-migration/known-diffs.md`, `scripts/admin-icons/SOURCES.md` and this file. To commit
  another markdown file, add an exception there. New settings go in `README.md`, `.env.example`
  and `docker-compose.yml` with their default.

## Hard rules (from upstream's README)

- An empty allowlist means nobody, never everybody.
- Five stars means Keep and is never destructive; Review actions stay behind a dry-run flag.
- Secrets never come back out of the admin API; logs redact `t s p apiKey token api_key client`.
- Octo never gets Docker access. The updater works through a file drop (`update/request` in the
  config folder) and a root systemd helper (`scripts/updater/`).
- Admin writes carry the `X-Octo-Admin: 1` header; keep the guard when adding endpoints.
- Never set `CPUS` above the machine's core count.

## Release and CI: what is dangerous

- Tags `v*` and `YYYY.MM.DD[.N]` (and every push to `main`) trigger `docker.yml`, which builds
  multi-arch images and pushes them to ghcr.io; a dated tag also publishes the GitHub release.
  `VERSION` must equal the tag and every `OCTO_IMAGE_TAG` default in `docker-compose.yml`.
  On this fork `docker.yml` and `release-links.yml` are **disabled in GitHub Actions**; `ci.yml`
  runs. Do not re-enable them without the owner's written OK.
- Compose files and `docker.yml` hardcode `ghcr.io/winters27/*`; a fork must retarget before it
  publishes anything.
- Version quirks: `VERSION` is dated (`2026.10.15`); the Cargo workspace is `2026.10.5` (no leading
  zero allowed); the displayed version comes from `OCTO_VERSION`, then `VERSION`.
- `parity/run.sh` needs a clean committed tree and heavy resources (WSL or an LXC box over ssh).
  `parity/lxc131.sh` and `host-watchdog.sh` talk to a Proxmox host. Only `parity/run.sh --selftest`
  is safe to run casually.
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
- Do not reformat or "clean up" code outside the task's scope.
- Use sonnet for subagents.
