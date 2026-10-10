# Modules

What a "module" is in Octo, so a new feature can be added, turned off, or removed without
touching other features. The agent policy (`AGENTS.md`, "Avoiding overlapping changes") points
here. This applies to new features (Wave 2 onward). Existing code is not moved to fit it; see
"How existing features compare" for where it differs today.

## What a module is

A module is one feature with these five parts.

| Part | Where | Rule |
|---|---|---|
| Code | `octo/Services/<Module>/` | All of the module's services, workers, stores and types. Namespace `Octo.Services.<Module>`. |
| Switch | env var `<MODULE>_ENABLED` (or a feature-specific name, see below) | One on/off switch, with its default written down in four places (below). |
| Tests | `octo.Tests/<Module>/` | The module's own test files, namespace `Octo.Tests.<Module>`. |
| Registration | one line in `octo/Program.cs` | `builder.Services.Add<Module>Module(builder.Configuration);` |
| Boundary | | No edits to another module's internals. Talk to other code through its public services or an extension point it already offers. |

Optional parts, when the module needs them:

- **HTTP endpoints:** its own controller file, `octo/Controllers/<Module>Controller.cs` (as
  `RadioAdminController.cs` and `SonicAdminController.cs` already do), not new actions in
  `SubSonicController*.cs` or `AdminController.cs`.
- **Dashboard page:** its own files under `octo/wwwroot`.
- **Settings class:** `octo/Services/<Module>/<Module>Settings.cs`, bound to its own
  configuration section `<Module>`. (Existing settings classes live in `octo/Models/Settings/`;
  new modules keep theirs in their folder so deleting the folder deletes them.)

### The registration line

Program.cs today registers everything inline, one feature's lines spread through the file.
A module instead puts its registrations in one extension method inside its folder:

```csharp
// octo/Services/<Module>/<Module>Module.cs
namespace Octo.Services.<Module>;

public static class <Module>Module
{
    public static IServiceCollection Add<Module>Module(this IServiceCollection services, IConfiguration config)
    {
        services.Configure<<Module>Settings>(config.GetSection("<Module>"));
        services.AddSingleton<...>();
        services.AddHostedService<...>();
        return services;
    }
}
```

and Program.cs gains exactly one line, placed with a one-line comment saying what the module
is. `Program.cs` is a shared hotspot (`AGENTS.md`), so this single line is the only edit a
module makes there. If the module plugs into an existing extension point (for example
`IRadioSource` or `INotificationSink`, both registered as "one more implementation"), that
registration goes in the module's extension method too, not in Program.cs.

### The switch

How a setting reaches Octo today, and so how a module's switch must be written:

1. `.env.example`: `<MODULE>_ENABLED=<default>` with a comment saying what it does.
2. `docker-compose.yml`, the `octo` service's environment:
   `<Module>__Enabled: ${<MODULE>_ENABLED:-<default>}`. The default after `:-` is what a user
   gets with no `.env` line.
3. `octo/appsettings.json`: a `"<Module>": { "Enabled": <default> }` section.
4. The settings class: `public bool Enabled { get; set; } = <default>;` with a doc comment
   naming the env var, as `RadioSourceSettings` does (`RADIO_SOUNDS_ALIKE`).

All four state the same default. `README.md` documents the switch and its default (required
for any PR that adds a setting, see `AGENTS.md`). The name can be feature-specific when that
reads better to users (`RADIO_SOUNDS_ALIKE`, `FOR_YOU_DEEP_CUTS`); what matters is one switch
per module, not the spelling.

**Check the switch at run time, not at registration.** The module's services are always
registered; they read `IOptionsMonitor<<Module>Settings>.CurrentValue.Enabled` when they run
and do nothing when it is off. This is how existing features behave, and it matters because
the dashboard writes settings to the settings file and Octo picks them up without a restart:

- `SoundsAlikeRadioSource.Available => settings.CurrentValue.SoundsAlike` drops the source
  from the blend the moment it is turned off.
- `SonicAnalysisWorker` holds with "Sounds alike is off" instead of stopping the host.

Do not write `if (config["X"] == "true") services.AddX();` in Program.cs: that freezes the
switch at start-up and makes other code that resolves the service fail when it is off.

### Off means off

With the switch off, the module makes no outgoing calls, starts no work, writes no files, and
its endpoints answer with a clear "off" result (not a 500). The rest of Octo behaves as if
the module were not there. Tests cover both states.

## Removing a module

1. Delete `octo/Services/<Module>/` and `octo.Tests/<Module>/`.
2. Delete its one registration line in `octo/Program.cs`.
3. Delete its switch: the `.env.example` line, the `docker-compose.yml` line, the
   `appsettings.json` section, and the README paragraph.
4. Delete its optional parts, if any: `octo/Controllers/<Module>Controller.cs`, its
   `octo/wwwroot` files.
5. `dotnet build` and `dotnet test` from the repo root still pass. If anything else fails to
   compile, some other code reached into the module's internals: that is a boundary bug to
   fix, not something to patch around.

A user's leftover `.env` line for a removed switch is harmless: nothing reads it.

## How a PR shows it touched only its module

The PR description lists the changed paths, grouped as:

- **Inside the module:** `octo/Services/<Module>/**`, `octo.Tests/<Module>/**`, and its own
  controller and `wwwroot` files.
- **Registration and switch (expected outside the module):** one added line in
  `octo/Program.cs`; one line each in `.env.example` and `docker-compose.yml`; one section in
  `octo/appsettings.json`; one README paragraph.
- **Anything else:** listed one by one with the reason. Each such file is a shared change, and
  hotspots among them need a claim first (`AGENTS.md`).

A reviewer can check it with:

```sh
git diff --stat origin/main...HEAD
git diff origin/main...HEAD -- octo/Program.cs   # expect one added line
```

## How existing features compare

Checked against **Radio** (multi-source radio, with Sounds alike) and **Made for you**
(New Releases, Rediscover, Deep Cuts). Both work well, but neither is a module in the sense
above. That is expected: they predate this convention. The convention is realistic because
every part of it already exists somewhere in these features; it only asks that a new feature
keep those parts together.

### Radio

What already fits:

- `octo/Services/Radio/` holds the multi-source radio code (`RadioSourceSet`, `RadioBlend`,
  `SongRadioService`, the `IRadioSource` implementations).
- Switches follow the four-place pattern: `RADIO_SOUNDS_ALIKE` in `.env.example`, mapped to
  `RadioSources__SoundsAlike` in `docker-compose.yml`, a `RadioSources` section in
  `appsettings.json`, and `RadioSourceSettings.SoundsAlike` with its default and env var named.
- Switches are read at run time through `IOptionsMonitor` (`SoundsAlikeRadioSource.Available`,
  `SonicAnalysisWorker`, `SongRadioService`).
- `IRadioSource` is a real extension point: a new source is one more implementation and one
  registration.
- Admin endpoints have their own controllers (`RadioAdminController`, `SonicAdminController`).

Where it does not follow the convention:

- **Spread over several folders:** `Services/Radio`, `Services/Sonic`, the `LastFmRadio*`
  services in `Services/LastFm`, `ListenBrainzRadioClient` in `Services/ListenBrainz`, and
  `RadioQueueStore` in `Services/Soulseek`. Removing Radio means editing all of them.
- **Not one registration:** its lines are spread through `Program.cs` (around lines 56-128,
  173, and 486-518 at the time of writing), interleaved with other features.
- **Settings outside the folder:** `RadioSourceSettings` is in `octo/Models/Settings/`.
- **Settings keys repeated in shared code:** `AdminController.cs` lists the `RadioSources`
  keys by hand in `GetSettings`, `GetRawConfig` and `GetConfigSources`, and holds the Sounds
  alike health probe. Adding a Radio setting means editing `AdminController.cs`.
- **Several switches, no single module switch:** `RADIO_YOUTUBE_MUSIC`, `RADIO_LISTENBRAINZ`,
  `RADIO_SOUNDS_ALIKE`, `RADIO_LEARN_FROM_LISTENING`. Radio as a whole cannot be turned off.
- **Tests are flat:** `octo.Tests/Radio*Tests.cs`, `Sonic*Tests.cs`, `LastFmRadio*Tests.cs`
  sit at the top of `octo.Tests/`, grouped only by file name. `octo.Tests/` has no
  per-feature folders yet; `octo.Tests/<Module>/` is new with this convention.
- **A sidecar:** Sounds alike also needs `octo-sonic` (`sonic/`, Rust) in `docker-compose.yml`,
  passed the same switch as `SONIC_ENABLED`. A module with its own container also has its own
  compose service, which must also honour the switch.

### Made for you

- **No folder of its own:** it lives in `octo/Services/Library/` (`ForYouLists.cs`, which also
  holds `NewReleasesBuilder`) and shares `GeneratedPlaylistService` with the genre and decade
  mixes.
- **Shares a settings section:** its switches `FOR_YOU_NEW_RELEASES`, `FOR_YOU_REDISCOVER`,
  `FOR_YOU_DEEP_CUTS` are properties of `GeneratedPlaylistSettings` (`GeneratedPlaylists`
  section), next to the mixes' settings. The four-place switch pattern is followed for each.
- **Served through shared files:** `SubSonicController.cs` (playlist listing and covers) and
  `SubsonicResponseBuilder.cs` (descriptions) call `ForYouLists` directly.
- Tests: `ForYouListsTests.cs`, `GeneratedPlaylistTests.cs`, flat.

Removing Made for you cleanly today would mean editing shared files in four places. Under
this convention it would be `octo/Services/ForYou/`, a `ForYou` settings section, and one
registration line.

### What this means for new modules

- A new module that extends Radio (for example another `IRadioSource`) can still be its own
  module: its folder, its switch, and an extension method that registers its `IRadioSource`.
- A new module that needs to appear in Subsonic responses (as Made for you does) has no
  extension point yet and has to edit `SubSonicController*.cs`, a hotspot. Keep that edit to
  a call into the module, and claim the hotspot first. Adding such an extension point is a
  separate issue, not part of a feature PR.
- Do not move existing features into modules as part of feature work (`AGENTS.md`: no
  clean-ups outside your issue's scope).

## The shared memory file

Agents coordinate in `.agents/memory.md`. The whole `/.agents/` folder is gitignored, so the
file is never committed; each working copy has its own. `AGENTS.md` says what goes in it.
If it does not exist, create it from this template:

```markdown
# Agent memory

Shared by all agents. Gitignored, never committed. No secrets, tokens, or personal data.
Rules: AGENTS.md. Template: docs/modules.md, "The shared memory file".

## Claims

<!-- - FLA-<n> | <role> agent | <branch> | <areas> | claimed <YYYY-MM-DD> -->

## Decisions

<!-- - <one line> (FLA-<n>) -->

## Open questions

<!-- - <question for a human> (FLA-<n>) -->
```

- **Claims:** one line per open claim. Add it when you set the issue to In Progress; remove it
  when the issue reaches Done or you release it.
- **Decisions:** one line each, with the issue it came from. Decisions that a later reader of
  the code needs belong in the repo's docs as well; this file is not the record.
- **Open questions:** questions waiting for a human, with the issue.

Check that the file is ignored with `git check-ignore -v .agents/memory.md`.
