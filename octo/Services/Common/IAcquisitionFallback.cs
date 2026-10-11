using System.Collections.Concurrent;
using Octo.Models.Settings;

namespace Octo.Services.Common;

/// <summary>Who started the chain run that ended without a file.</summary>
public enum MissOrigin { Heart, Trickle }

/// <summary>
/// A song whose whole chain of enabled sources ran and came back empty. Steps are the sources the
/// run used, in order.
/// </summary>
public sealed record ChainMiss(string Provider, string ExternalId, string? RequestedBy, MissOrigin Origin,
    IReadOnlyList<HeartDownloadSource> Steps, DateTime? StartedUtc = null);

/// <summary>What the fallback did with a miss. Declined: nothing, the row fails as it always did.
/// Obtained: the file arrived by another way. Parked: not yet, the fallback will have the chain run
/// again, so the row stays open.</summary>
public enum FallbackResult { Declined, Obtained, Parked }

/// <summary>
/// The one place a feature can act on a song the chain could not get. Offered once at the end of
/// <see cref="HeartAcquisitionCoordinator.AcquireTrackAsync"/>, which only hearts and the import
/// trickle call, so albums, plays and playlist fetches never reach it. With none registered the
/// chain behaves exactly as it did before.
/// </summary>
public interface IAcquisitionFallback
{
    /// <summary>True when this fallback may handle a run of these sources. Asked before the run
    /// starts, so a failure notice is held back only when the fallback may still fetch the song.</summary>
    bool Applies(IReadOnlyList<HeartDownloadSource> steps);

    Task<FallbackResult> OfferAsync(ChainMiss miss, CancellationToken ct = default);

    /// <summary>A person took their heart back: forget what is counted for that song and person,
    /// so nothing is retried or fetched for it.</summary>
    void Forget(string provider, string externalId, string? requestedBy);
}

/// <summary>
/// Downloads a feature asked for in its own name, by external id, so the download can write what
/// the feature wants recorded in the file's tags without a new parameter on every download call.
/// The value is a constant, never a person.
/// </summary>
public static class AcquisitionMarks
{
    public const string YouTubeFallback = "youtube-fallback";

    private static readonly ConcurrentDictionary<string, string> Marks = new(StringComparer.Ordinal);

    public static void Mark(string externalId, string mark) => Marks[externalId] = mark;
    public static void Unmark(string externalId) => Marks.TryRemove(externalId, out _);
    public static string? Get(string? externalId) =>
        externalId is not null && Marks.TryGetValue(externalId, out var mark) ? mark : null;
}
