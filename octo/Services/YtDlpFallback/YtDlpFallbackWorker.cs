using Microsoft.Extensions.Options;
using Octo.Services.Common;
using Octo.Services.Soulseek;

namespace Octo.Services.YtDlpFallback;

/// <summary>
/// Runs the chain again for hearts the fallback is counting: when the spacing since the last
/// counted attempt has passed, or when Soulseek is back after an outage. Gives a heart up when it
/// has waited MaxWaitHours. An entry the chain run did not touch means the song arrived another
/// way, and it is dropped.
///
/// Import trickle songs are not run from here; the trickle brings them back itself.
/// </summary>
public sealed class YtDlpFallbackWorker : BackgroundService
{
    internal static readonly TimeSpan PollEvery = TimeSpan.FromSeconds(30);

    private readonly IOptionsMonitor<YtDlpFallbackSettings> _settings;
    private readonly YtDlpFallbackStore _store;
    private readonly HeartAcquisitionCoordinator _hearts;
    private readonly AcquisitionTracker? _tracker;
    private readonly ISoulseekLink? _soulseek;
    private readonly ILogger<YtDlpFallbackWorker> _logger;

    public YtDlpFallbackWorker(IOptionsMonitor<YtDlpFallbackSettings> settings, YtDlpFallbackStore store,
        HeartAcquisitionCoordinator hearts, ILogger<YtDlpFallbackWorker> logger,
        AcquisitionTracker? tracker = null, ISoulseekLink? soulseek = null,
        Octo.Services.Imports.TrickleQueue? trickle = null, YtDlpFallbackService? service = null)
    {
        _settings = settings;
        _store = store;
        _hearts = hearts;
        _tracker = tracker;
        _soulseek = soulseek;
        _logger = logger;
        // Import songs that were not found come back after the spacing, not two weeks, until they
        // have had their counted tries. Only while the fallback would act on them.
        if (trickle is not null && service is not null)
            trickle.RetryAfter = job =>
            {
                var current = settings.CurrentValue.Normalized();
                return service.AppliesNow() && job.Attempts < current.FailedAttempts
                    ? TimeSpan.FromHours(current.AttemptSpacingHours)
                    : Octo.Services.Imports.TrickleQueue.RetryNotFoundAfter;
            };
    }

    internal Func<DateTime> Clock { get; set; } = () => DateTime.UtcNow;

    // A chain can wait hours for Soulseek, so runs go on beside the loop, one per song.
    private readonly System.Collections.Concurrent.ConcurrentDictionary<string, byte> _running = new();
    private readonly System.Collections.Concurrent.ConcurrentBag<Task> _runs = new();

    /// <summary>Waits for the chains started so far. Only tests need it.</summary>
    internal Task DrainAsync() => Task.WhenAll(_runs.ToArray());

    private async Task RunAsync(FallbackEntry entry, DateTime startedAt)
    {
        try
        {
            await _hearts.AcquireTrackAsync(entry.Provider, entry.ExternalId,
                entry.RequestedBy.Length == 0 ? null : entry.RequestedBy, heldSinceUtc: null, MissOrigin.Heart);
            // The run offered the song again if it ended empty; if it did not touch the entry, the
            // song arrived (or the fallback stood down) and there is nothing left to count.
            if (_store.Get(entry.Provider, entry.ExternalId, entry.RequestedBy) is { } after && after.LastRunUtc < startedAt)
                _store.Remove(after.Key);
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            _logger.LogWarning("A heart the fallback was counting failed to run ({Type})", ex.GetType().Name);
        }
        finally { _running.TryRemove(entry.Key, out _); }
    }

    /// <summary>One pass. Answers how many chains it started. With the switch off it does nothing.</summary>
    internal async Task<int> TickAsync(CancellationToken ct)
    {
        var settings = _settings.CurrentValue.Normalized();
        if (!settings.Enabled) return 0;
        var now = Clock();
        var started = 0;
        foreach (var entry in _store.Snapshot())
        {
            ct.ThrowIfCancellationRequested();
            if (now - entry.FirstMissUtc >= TimeSpan.FromHours(settings.MaxWaitHours))
            {
                // A heart has a row to close; a trickle song just stops being counted.
                if (_store.Remove(entry.Key, givenUp: true) && entry.Origin == "heart")
                    _tracker?.Fail(entry.Provider, entry.ExternalId,
                        $"Not found anywhere after {entry.CountedAttempts} of {settings.FailedAttempts} tries over {settings.MaxWaitHours} hours.");
                continue;
            }
            if (entry.Origin != "heart") continue;

            var spacingDue = entry.LastCountedUtc is null
                || now - entry.LastCountedUtc.Value >= TimeSpan.FromHours(settings.AttemptSpacingHours);
            if (!spacingDue || now - entry.LastRunUtc < PollEvery) continue;
            // Parked for an outage: wait until Soulseek reads healthy, so a run is not wasted.
            if (entry.CountedAttempts == 0 && !await SoulseekUpAsync(ct)) continue;

            if (!_running.TryAdd(entry.Key, 0)) continue;
            started++;
            var task = Task.Run(() => RunAsync(entry, now), CancellationToken.None);
            _runs.Add(task);
        }
        return started;
    }

    private async Task<bool> SoulseekUpAsync(CancellationToken ct)
    {
        if (_soulseek is null) return false;
        try { return (await _soulseek.ReadAsync(fresh: true, ct))?.Link == SoulseekLinkState.LoggedIn; }
        catch (Exception ex) when (ex is not OperationCanceledException) { return false; }
    }

    protected override async Task ExecuteAsync(CancellationToken stoppingToken)
    {
        while (!stoppingToken.IsCancellationRequested)
        {
            try { await TickAsync(stoppingToken); }
            catch (OperationCanceledException) when (stoppingToken.IsCancellationRequested) { break; }
            catch (Exception ex) { _logger.LogError("The yt-dlp fallback worker failed a pass ({Type})", ex.GetType().Name); }
            try { await Task.Delay(PollEvery, stoppingToken); } catch (OperationCanceledException) { break; }
        }
    }
}
