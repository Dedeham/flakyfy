using Microsoft.Extensions.Options;
using Octo.Models.Settings;
using Octo.Services.Common;
using Octo.Services.Soulseek;

namespace Octo.Services.YtDlpFallback;

/// <summary>
/// Counts the chain runs that ended empty and, once there are enough of them, fetches the song
/// from YouTube as an MP3. It acts only when the switch is on and the person's own source list has
/// no YouTube step (they already chose YouTube themselves, in its place).
///
/// What counts as an attempt: a whole run of the enabled sources that ended without a file while
/// Soulseek was logged in before and after it, at least AttemptSpacingHours after the previous
/// counted one. A Soulseek outage never counts; a Lidarr failure does. Nothing here gives up on a
/// song because of an outage: it waits, and the worker runs the chain again.
/// </summary>
public sealed class YtDlpFallbackService : IAcquisitionFallback
{
    private readonly IOptionsMonitor<YtDlpFallbackSettings> _settings;
    private readonly YtDlpFallbackStore _store;
    private readonly TrackAcquisitionQueue _queue;
    private readonly ISoulseekLink? _soulseek;
    private readonly ILogger<YtDlpFallbackService> _logger;
    private readonly SemaphoreSlim _oneAtATime = new(1, 1);
    private readonly object _budgetLock = new();
    private readonly List<(DateTime At, string User)> _started = [];
    private readonly string _downloadPath;
    private readonly IOptionsMonitor<SubsonicSettings>? _subsonic;

    public YtDlpFallbackService(IOptionsMonitor<YtDlpFallbackSettings> settings, YtDlpFallbackStore store,
        TrackAcquisitionQueue queue, IConfiguration configuration, ILogger<YtDlpFallbackService> logger,
        ISoulseekLink? soulseek = null, IOptionsMonitor<SubsonicSettings>? subsonic = null)
    {
        _subsonic = subsonic;
        _settings = settings;
        _store = store;
        _queue = queue;
        _soulseek = soulseek;
        _logger = logger;
        _downloadPath = configuration["Library:DownloadPath"] ?? "./downloads";
        FreeBytes = path => new DriveInfo(Path.GetFullPath(path)).AvailableFreeSpace;
    }

    // Seams for tests.
    internal Func<DateTime> Clock { get; set; } = () => DateTime.UtcNow;
    internal Func<string, long> FreeBytes { get; set; }

    internal YtDlpFallbackSettings Current => _settings.CurrentValue.Normalized();

    public bool Applies(IReadOnlyList<HeartDownloadSource> steps) =>
        Current.Enabled && !steps.Contains(HeartDownloadSource.YouTube);

    /// <summary>True when the fallback would act on a song hearted now: it is on and the song
    /// sources in use have no YouTube step.</summary>
    public bool AppliesNow() =>
        _subsonic is not null && Applies(_subsonic.CurrentValue.EffectiveHeartDownloadSources()
            .Where(step => step.SongEnabled == true).Select(step => step.Source).ToList());

    public async Task<FallbackResult> OfferAsync(ChainMiss miss, CancellationToken ct = default)
    {
        var settings = Current;
        // Off, or YouTube already in the person's own list: nothing here, no file, no call.
        if (!settings.Enabled || miss.Steps.Contains(HeartDownloadSource.YouTube)) return FallbackResult.Declined;

        var heart = miss.Origin == MissOrigin.Heart;
        var now = Clock();
        var by = miss.RequestedBy ?? "";
        var existing = _store.Get(miss.Provider, miss.ExternalId, by);

        // A new song needs room: one person's pile cannot crowd out another's or grow the file forever.
        if (existing is null && (_store.CountFor(by) >= YtDlpFallbackSettings.MaxEntriesPerUser
                                 || _store.Count >= YtDlpFallbackSettings.MaxEntries))
        {
            _logger.LogInformation("Too many songs are waiting for the yt-dlp fallback; not adding another");
            return FallbackResult.Declined;
        }
        var entry = existing ?? new FallbackEntry(miss.Provider, miss.ExternalId, by, heart ? "heart" : "trickle",
            now, 0, null, now);

        // An outage never counts: Soulseek must be logged in, for sure, right now. No answer
        // (null) and Unknown are outages.
        if (miss.Steps.Contains(HeartDownloadSource.Soulseek) && !await SoulseekIsUpAsync(ct))
        {
            _store.Put(entry with { LastRunUtc = now });
            return heart ? FallbackResult.Parked : FallbackResult.Declined;
        }

        // One counted attempt per spacing, however often the song is hearted or retried.
        var spacing = TimeSpan.FromHours(settings.AttemptSpacingHours);
        if (entry.LastCountedUtc is not { } lastCounted || now - lastCounted >= spacing)
            entry = entry with { CountedAttempts = Math.Min(entry.CountedAttempts + 1, YtDlpFallbackStore.MaxCount), LastCountedUtc = now };
        entry = entry with { LastRunUtc = now };
        _store.Put(entry);

        if (entry.CountedAttempts < settings.FailedAttempts)
            return heart ? FallbackResult.Parked : FallbackResult.Declined;

        // Eligible. Last checks, then one download at a time.
        if (!await _oneAtATime.WaitAsync(0, ct)) return heart ? FallbackResult.Parked : FallbackResult.Declined;
        try
        {
            if (miss.Steps.Contains(HeartDownloadSource.Soulseek) && !await SoulseekIsUpAsync(ct))
                return heart ? FallbackResult.Parked : FallbackResult.Declined;
            if (!HasRoom(by, now)) return heart ? FallbackResult.Parked : FallbackResult.Declined;
            return await DownloadAsync(miss, entry, heart, now, ct);
        }
        finally { _oneAtATime.Release(); }
    }

    private async Task<bool> SoulseekIsUpAsync(CancellationToken ct)
    {
        if (_soulseek is null) return false;
        try { return (await _soulseek.ReadAsync(fresh: true, ct))?.Link == SoulseekLinkState.LoggedIn; }
        catch (Exception ex) when (ex is not OperationCanceledException) { return false; }
    }

    private bool HasRoom(string user, DateTime now)
    {
        try
        {
            if (FreeBytes(_downloadPath) < YtDlpFallbackSettings.MinFreeBytes)
            {
                _logger.LogWarning("Not enough free space for the yt-dlp fallback; waiting");
                return false;
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning("The free space for the yt-dlp fallback could not be read ({Type}); waiting", ex.GetType().Name);
            return false;
        }
        lock (_budgetLock)
        {
            _started.RemoveAll(s => now - s.At >= TimeSpan.FromHours(1));
            if (_started.Count >= YtDlpFallbackSettings.PerHour
                || _started.Count(s => string.Equals(s.User, user, StringComparison.OrdinalIgnoreCase)) >= YtDlpFallbackSettings.PerUserPerHour)
                return false;
            _started.Add((now, user));
            return true;
        }
    }

    private async Task<FallbackResult> DownloadAsync(ChainMiss miss, FallbackEntry entry, bool heart, DateTime now, CancellationToken ct)
    {
        AcquisitionMarks.Mark(miss.ExternalId, AcquisitionMarks.YouTubeFallback);
        try
        {
            await _queue.Enqueue(miss.Provider, miss.ExternalId, isStar: true, triggerAlbumDownload: false,
                forcePermanent: true, sourceOverride: DownloadSource.YouTube, notifyOnFailure: false,
                requestedBy: miss.RequestedBy);
            _store.Remove(entry.Key);
            _logger.LogInformation("The yt-dlp fallback fetched a song after {Count} counted attempts", entry.CountedAttempts);
            return FallbackResult.Obtained;
        }
        catch (Exception ex) when (!ct.IsCancellationRequested)
        {
            // No song names in this line: they are in the row the person sees.
            _logger.LogWarning("The yt-dlp fallback could not get a song ({Type})", ex.GetType().Name);
            return heart ? FallbackResult.Parked : FallbackResult.Declined;
        }
        finally { AcquisitionMarks.Unmark(miss.ExternalId); }
    }
}
