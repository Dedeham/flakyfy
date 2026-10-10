# Agent orchestration policy

How AI agents pick up, carry out, and hand back work on Flakyfy (the Octo fork). Work is
tracked in Linear: workspace `cv-agent`, team **flakyfy** (`FLA`), project **Flakyfy**.
Everything here applies to every agent, whatever tool runs it.

**Agents never push to `main`, never push tags, and never publish or deploy.** Only the
**orchestrator** (the agent that hands out the issues) may merge, and only a PR that was
verified as below. Issue agents and verifiers never merge. In this repository a tag cuts a
release that installs pull through the update helper. `docker.yml`, which builds and
publishes images to ghcr.io on every push to `main`, is disabled in GitHub Actions. While it
stays disabled, a merge runs only CI. Whoever turns it back on makes every merge a
deployment, and must say so here first.

The human may also decide things in the orchestrator's chat instead of in Linear. The
orchestrator then posts the decision on the issue or PR, quoting the human's words, with the
date and the scope. Such a quoted relay counts as the human's written approval.

## Roles

Each issue has exactly one owner role, set by its label in the **Agent** label group. Other
roles named under "Supports" in the issue help but do not own it.

| Role | Label | Responsible for |
|---|---|---|
| Architect | `Architect` | Architecture docs, research, designs, module boundaries, the detailed plan. Writes docs, not feature code. |
| Backend developer | `Backend` | Code in `octo/` (.NET 9), `sonic/` (Rust), `yt-dlp-shim/` (Python), compose and updater scripts. |
| Frontend developer | `Frontend` | The dashboard and pages in `octo/wwwroot`, and badge/cover assets. |
| QA engineer | `QA` | Test fixtures, the integration harness, CI test jobs, and checking other roles' acceptance criteria. |
| Security reviewer | `Security` | Security requirements, audits, and review of every PR that touches auth, user data, credentials, external input, or file deletion. |

## Which model runs a task

Every task runs on **Claude Sonnet** (the agent tool's `sonnet` model), including
architecture, research, security reviews and the gate plan. The human decided this on
2026-10-10, replacing the earlier Sonnet/Opus split. Only the human may move a task to a
different model; say so in a comment. If a task turns out bigger than one agent can carry,
stop, comment why, and ask the orchestrator to split it instead of pushing on.

## Picking up work

| Rule | Behavior |
|---|---|
| Ready to execute | Status is **Todo**, every issue in "blocked by" is **Done**, and there is no `Blocked` label. Anything in **Backlog** is not ready. |
| Assignment | Take only issues whose Agent label matches your role. |
| Concurrency | Start independent ready issues in parallel, one agent per issue, as long as their claimed areas don't overlap (see below). |
| Progress | Set the issue to **In Progress** and post a claim comment before changing any file. |
| Completion | Mark **Done** only after verification passes and the PR is merged by the orchestrator or a human (see below). |
| Failure | Record the error on the issue. Retry only transient failures, at most twice. |
| Blocked | Stop. Add the `Blocked` label, say why in a comment, and do not start anything that depends on this issue. |

### Getting a task

1. List the project's issues in **Todo** with your role's label (Linear MCP: `list_issues` with
   project `Flakyfy`, state `Todo`, label = your role).
2. For each candidate, read the issue and its relations (`get_issue`). Skip it if any blocker is
   not **Done**, if it has the `Blocked` label, or if another agent has claimed it in the last
   24 hours without a release comment.
3. Pick the highest priority. On a tie, pick the lowest issue number.
4. Read the shared memory file (see "Reporting progress") for decisions and claims that touch
   your area. If it doesn't exist yet, create it from the template in `docs/modules.md`.
5. While looking, promote any **Backlog** issue whose blockers are now all **Done** to
   **Todo**, and comment that it is ready.

### Checking dependencies

- Dependencies are Linear "blocked by" relations. The Dependencies section in the issue text
  explains them but is not the source of truth.
- Check again right before opening the PR. If a blocker was reopened, stop and add `Blocked`.
- An issue in **In Review** does not unblock anything. Its dependents become ready only when it
  is **Done**, which means its PR is merged. The orchestrator, or Linear's GitHub integration on
  merge, sets **Done**. The next agent to pick up work promotes the issues it unblocked (pickup
  step 5).
- Wave 2 build issues depend on **FLA-17**, which needs written human approval. No agent may
  close FLA-17 without that approval quoted in a comment.

## Reporting progress

| When | Linear status | Comment |
|---|---|---|
| Start | In Progress | `[<Role> agent] Claimed. Branch: <branch>. Areas: <folders/files>.` |
| Notable finding or change of plan | unchanged | One or two sentences: what changed and why. |
| PR opened | In Review | PR link, plus each acceptance criterion with its evidence. |
| Verified and merged | Done | Who verified it and a link to the CI run. |
| Blocked or failed | Todo + `Blocked` label | See "Failed tasks". |
| Giving up a claim | Todo | `[<Role> agent] Released: <reason>.` |

Also keep the shared memory file up to date. It is gitignored and never committed. Every agent
on this machine uses the **main checkout's** copy, even from a worktree:
`"$(git rev-parse --path-format=absolute --git-common-dir)/../.agents/memory.md"`. Agents in
other clones or on other machines can't see it, so **the claim comment in Linear is what
counts**. The memory file is a local convenience. It holds:

- **Claims:** issue, agent, branch, areas.
- **Decisions:** one line each, with the issue they came from.
- **Open questions** for a human.

The author removes their own claim when the issue reaches **Done**, or when they release it.
Verifiers never remove claims.

Every comment an agent posts starts with its role, for example `[Architect agent]` or
`[QA agent]`. All agents post as the same Linear user, so this tag is the only thing that
shows who wrote what. Don't put secrets,
tokens, or personal data in it.

## Verifying acceptance criteria

Every issue has a checklist of acceptance criteria, and each item needs evidence:

- A test name and the CI run that passed it.
- A doc path.
- A screenshot or recording for UI work, checked in a browser at desktop and phone width.
- Command output, where that is the right proof.

1. **The author checks** that every checkbox is ticked and has evidence, and that the existing
   checks pass locally:
   - `dotnet test` from the repo root
   - `pytest yt-dlp-shim/tests -q`
   - in `sonic/`: `cargo clippy --all-targets -- -D warnings` and `cargo test`
   - the integration harness, once FLA-12 has landed
2. **A different agent verifies.** QA by default. Security as well when the issue says Security
   must approve, or when the change touches auth, user data, credentials, external input, or
   deleting files. The verifier reruns the evidence, ticks each acceptance-criteria checkbox that holds, and
   comments `Verified` or lists what fails.
   An agent never verifies its own work.
3. **The orchestrator (or a human) merges.** The issue moves to **Done** only after the merge.

For docs-only work there is nothing to rerun. Instead, the verifier checks at least two of the
doc's concrete claims against the code and names the files it checked.

Evidence must be on the issue's branch. A criterion that depends on files that are uncommitted,
untracked, or only on another branch fails. The author commits those files to the branch, or
the human commits them to `main` first.

Research and design issues are checked the same way: the doc is delivered in a PR, and the
Architect's research is verified by QA (is it complete against the criteria?) and, for
designs, by Security.

## Failed tasks

- Post a comment with what you were doing, the command, a short excerpt of the error (never
  secrets), how many attempts so far, and your diagnosis.
- **Retry only transient failures**, at most two more times: network timeouts, a registry or
  service outage, a CI runner failing for infrastructure reasons.
- **Do not retry blindly:** a failing assertion, a build error from your change, a policy stop,
  or anything needing a human decision. Fix the cause or mark the issue Blocked.
- Never get a check to pass by skipping or deleting tests, adding `[skip ci]`, weakening
  assertions, or turning off linters.
- After the retry limit, or if you can't fix it: add the `Blocked` label, move the issue back to
  **Todo**, keep the branch, and release your claim in `.agents/memory.md`.

## Avoiding overlapping changes

- One issue, one agent, one branch. Don't edit files outside the areas you claimed without
  updating your claim first.
- Work in your own git worktree (`git worktree add ../flakyfy-fla-<n> -b <branch> origin/main`).
  Never switch the branch of the main checkout: other agents and the human are using it.
- New features go in their own module, as described in `docs/modules.md` (from FLA-8): their own
  folder under `octo/Services/<Module>`, their own config switch, their own tests, and a single
  registration line.
- **Shared hotspots** need a claim before you edit them. Change only what registers or documents
  your module there: the registration line(s) in `Program.cs`, your setting's lines in
  `.env.example` and `appsettings*.json`, your own README section, or your own CI job. Hotspots:
  - `octo/Program.cs`
  - `octo/Controllers/SubSonicController*.cs`
  - `docker-compose.yml`, `.env.example`
  - `README.md`
  - `.github/workflows/*`
  - `octo/appsettings*.json`

  If another open claim already covers a hotspot you need, coordinate in that issue's comments,
  or wait.
- Don't reformat, rename, or "clean up" code outside your issue's scope. Open a new issue
  instead.
- Rebase on `origin/main` before opening a PR and again before asking for review. Resolve
  conflicts. Never discard someone else's change to make yours apply.

## Branches, commits and pull requests

- **Branch:** `<type>/fla-<n>-<short-slug>` from an up-to-date `main`, for example
  `feat/fla-18-ytmusic-import` or `fix/fla-10-fetch-layout-jump`. The FLA id links the branch to
  the Linear issue.
- **Types** follow the existing history: `feat`, `fix`, `perf`, `test`, `docs`, `ci`.
- **Commits** follow the repo's style, `type(scope): <what is now true, in plain words>`.
  Examples: `fix(install): the health check waits for services that are still starting`, or for
  docs, `docs(modules): a module is one folder, one switch and one registration line`. Use a
  scope that names the area (`imports`, `search`, `admin`, `updater`, ...). Make small commits
  that each build. Docs-only commits may add `[skip ci]`; no other commit may.
- **No AI attribution:** no `Co-Authored-By` trailers naming Claude or Anthropic, no
  "Generated with Claude Code" lines, in commits, PR descriptions, or files.
- **Never:** commit secrets or `.env` (use `.env.example` for new settings), force-push a
  branch you don't own, rewrite `main`, or skip hooks.
- **Pull requests:**
  - Base is `main`.
  - Title follows the commit style. The body links the Linear issue (`FLA-<n>`) and lists each
    acceptance criterion with its evidence.
  - Open the PR as a draft until the local checks pass, then mark it ready and move the issue
    to **In Review**.
  - CI must be green.
  - A PR that adds a setting documents it in `README.md` and `.env.example`, and states its
    default.

## Human approval

Agents must stop and get explicit written approval from a human (in the Linear issue or the PR)
before doing any of the following:

- Pushing to `main` directly. The orchestrator may merge verified PRs without asking each
  time; the human granted this on 2026-10-10.
- Turning `docker.yml` back on in GitHub Actions.
- Creating or pushing a tag, creating or publishing a GitHub release, or running the
  `docker.yml` or `release-links.yml` workflows by hand.
- Deploying, or running the update helper (`scripts/updater/`) or `install.sh` against any real
  server.
- Changing repository settings, branch protection, Actions secrets, or ghcr.io package
  visibility.
- Closing the gate issue **FLA-17**, which approves the stack and Wave 2 scope.
- Adding a new external service, container, or third-party dependency that the approved plan
  doesn't list.
- Anything that deletes user data outside a test stack, including running the retention cleanup
  without dry-run.
- Accepting a Critical or High security finding as "won't fix".

Approval covers only the action and scope it names. An earlier approval does not carry over to
a later action.
