using System.Collections.Concurrent;
using Microsoft.Extensions.Options;
using Octo.Models.Settings;
using Octo.Services.Common;
using Octo.Services.Subsonic;

namespace Octo.Services.Imports;

/// <summary>
/// Makes imported liked songs favorites in Navidrome, as the person who imported them (the
/// IMPORTS_STAR_LIKED switch). Navidrome only favorites as the user who signs the call, and Octo
/// holds no sign-in for users, so the import only queues song ids (ImportStore.QueueStars). The
/// next time that person's app makes a request Navidrome accepts, this runs on it, favorites a
/// batch through StarOnArrival.StarSongAsync (the one way Octo favorites as a user) and lets the
/// credential go. A big list takes a few requests rather than one long burst.
///
/// The credential is a local value of the task started here: never stored, and its password, if
/// the app sent one, is swapped for a token first.
/// </summary>
public sealed class ImportStarsOnRequest : IVerifiedRequestObserver
{
    /// <summary>Favorites set per request, so a list of thousands trickles in over a few requests.</summary>
    internal const int PerRequest = 100;

    private readonly ImportStore _store;
    private readonly StarOnArrival _stars;
    private readonly IOptionsMonitor<ImportSettings> _settings;
    private readonly ILogger<ImportStarsOnRequest> _logger;
    private readonly ConcurrentDictionary<string, byte> _running = new(StringComparer.OrdinalIgnoreCase);

    public ImportStarsOnRequest(ImportStore store, StarOnArrival stars, IOptionsMonitor<ImportSettings> settings,
        ILogger<ImportStarsOnRequest> logger)
    {
        _store = store; _stars = stars; _settings = settings; _logger = logger;
    }

    /// <summary>The work of the last request, for tests to wait on.</summary>
    internal Task Last { get; private set; } = Task.CompletedTask;

    public void OnVerifiedRequest(string user, SubsonicCredential credential)
    {
        if (!_settings.CurrentValue.StarLiked || !_store.HasStars(user)) return;
        // One run per person at a time, or two requests would favorite the same songs twice.
        if (!_running.TryAdd(user, 0)) return;
        var token = credential.WithoutPassword();
        using (ExecutionContext.SuppressFlow())
            Last = Task.Run(() => StarAsync(user, token));
    }

    private async Task StarAsync(string user, SubsonicCredential credential)
    {
        try
        {
            var ids = _store.TakeStars(user, PerRequest);
            if (ids.Count == 0) return;
            var failed = new List<string>();
            foreach (var id in ids)
                if (!await _stars.StarSongAsync(credential, user, id)) failed.Add(id);
            if (failed.Count > 0) _store.RequeueStars(user, failed);
            _logger.LogInformation("Favorited {Count} imported liked song(s) for {User}{Failed}", ids.Count - failed.Count, user,
                failed.Count > 0 ? $"; {failed.Count} will be tried again on their next request" : "");
        }
        catch (Exception ex) { _logger.LogWarning("Could not favorite imported liked songs for {User}: {Reason}", user, ex.Message); }
        finally { _running.TryRemove(user, out _); }
    }
}
