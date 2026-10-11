using Microsoft.Extensions.Logging.Abstractions;
using Moq;
using Octo.Models.Settings;
using Octo.Services;
using Octo.Services.Common;
using Octo.Services.Lidarr;

namespace Octo.Tests.YtDlpFallback;

/// <summary>The fallback as the heart chain meets it: where it is offered, and where it never is.</summary>
public sealed class YtDlpFallbackChainTests
{
    private static SubsonicSettings Chain(bool youTube) => new()
    {
        HeartDownloadSources =
        [
            new() { Source = HeartDownloadSource.Soulseek, Enabled = true },
            new() { Source = HeartDownloadSource.YouTube, Enabled = youTube },
            new() { Source = HeartDownloadSource.Lidarr, Enabled = true },
        ],
    };

    private sealed class Rig : IDisposable
    {
        public FallbackRig Fallback = new();
        public Mock<ILidarrHeartAcquisitionService> Lidarr = new();
        public Mock<IDownloadService> Direct = new();
        public TrackAcquisitionQueue Queue = new(NullLogger<TrackAcquisitionQueue>.Instance);
        public AcquisitionTracker Tracker = new(NullLogger<AcquisitionTracker>.Instance);
        public HeartAcquisitionCoordinator Hearts;
        public List<AcquisitionRequest> Seen = [];
        public bool SoulseekFails = true;
        private readonly CancellationTokenSource _stop = new();

        public Rig(bool youTube = false, IEnumerable<IAcquisitionFallback>? fallbacks = null)
        {
            Hearts = new HeartAcquisitionCoordinator(TestOptions.Monitor(Chain(youTube)), Queue, Direct.Object,
                Lidarr.Object, NullLogger<HeartAcquisitionCoordinator>.Instance, tracker: Tracker,
                fallbacks: fallbacks ?? [Fallback.Service]);
            // The download worker: Soulseek finds nothing; anything else lands.
            _ = Task.Run(async () =>
            {
                try
                {
                    while (!_stop.IsCancellationRequested)
                    {
                        var request = await Queue.DequeueAsync(_stop.Token);
                        if (request is null) continue;
                        lock (Seen) Seen.Add(request);
                        Queue.Release(request);
                        if (request.SourceOverride == DownloadSource.Soulseek && SoulseekFails)
                            request.Completion.TrySetException(new FileNotFoundException("no peer"));
                        else request.Completion.TrySetResult("/music/x.mp3");
                    }
                }
                catch (OperationCanceledException) { }
            });
        }

        public void Dispose() { _stop.Cancel(); Fallback.Dispose(); }
        public DownloadSource?[] Sources() { lock (Seen) return Seen.Select(r => r.SourceOverride).ToArray(); }
    }

    [Fact]
    public async Task AHeartThatEverySourceFailsIsParkedNotFailed_AndThirdTimeItArrivesFromYouTube()
    {
        using var rig = new Rig();
        rig.Tracker.Begin("soulseek", "song-1", null, "alice", "A", "T", null);

        await rig.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");
        var row = rig.Tracker.All().Single();
        Assert.NotEqual(AcquisitionState.Failed, row.State); // waiting, not failed
        rig.Fallback.Now += TimeSpan.FromHours(6);
        await rig.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");
        Assert.Empty(rig.Fallback.Requests);
        rig.Fallback.Now += TimeSpan.FromHours(6);

        await rig.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");

        // The chain's own sources were asked three times and never YouTube; the fallback asked once.
        Assert.DoesNotContain(DownloadSource.YouTube, rig.Sources().Where(s => s is not null).Select(s => s!.Value));
        Assert.Equal(DownloadSource.YouTube, Assert.Single(rig.Fallback.Requests).SourceOverride);
        rig.Lidarr.Verify(l => l.TryAcquireTrackAsync("soulseek", "song-1", false, "alice"), Times.Exactly(3));
    }

    [Fact]
    public async Task TheFailureNoticeIsHeldBackOnlyWhenTheFallbackMayStillFetchTheSong()
    {
        using var withFallback = new Rig();
        await withFallback.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");
        withFallback.Lidarr.Verify(l => l.TryAcquireTrackAsync(It.IsAny<string>(), It.IsAny<string>(), true, It.IsAny<string?>()), Times.Never);

        using var without = new Rig(fallbacks: []);
        await without.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");
        without.Lidarr.Verify(l => l.TryAcquireTrackAsync("soulseek", "song-1", true, "alice"), Times.Once);
    }

    [Fact]
    public async Task WithTheYouTubeStepOnTheFallbackIsNeverOffered()
    {
        var spy = new Mock<IAcquisitionFallback>();
        spy.Setup(f => f.Applies(It.IsAny<IReadOnlyList<HeartDownloadSource>>()))
            .Returns<IReadOnlyList<HeartDownloadSource>>(steps => !steps.Contains(HeartDownloadSource.YouTube));
        using var rig = new Rig(youTube: true, fallbacks: [spy.Object]);
        await rig.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");
        spy.Verify(f => f.OfferAsync(It.IsAny<ChainMiss>(), It.IsAny<CancellationToken>()), Times.Never);
        Assert.Equal(AcquisitionState.Failed, rig.Tracker.All().FirstOrDefault()?.State ?? AcquisitionState.Failed);
    }

    [Fact]
    public async Task ARealServiceStaysOutOfAChainThatHasYouTube()
    {
        using var rig = new Rig(youTube: true);
        for (var i = 0; i < 4; i++)
        {
            await rig.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");
            rig.Fallback.Now += TimeSpan.FromHours(7);
        }
        Assert.False(File.Exists(rig.Fallback.StorePath));
        Assert.Empty(rig.Fallback.Requests);
    }

    [Fact]
    public async Task OffMeansTheChainFailsTheRowExactlyAsBefore()
    {
        using var rig = new Rig(fallbacks: []);
        rig.Tracker.Begin("soulseek", "song-1", null, "alice", "A", "T", null);
        await rig.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");
        Assert.Equal(AcquisitionState.Failed, rig.Tracker.All().Single().State);
        Assert.False(File.Exists(rig.Fallback.StorePath));
    }

    [Fact]
    public async Task AnAlbumHeartAndAPlayAreNeverOffered()
    {
        var spy = new Mock<IAcquisitionFallback>();
        spy.Setup(f => f.Applies(It.IsAny<IReadOnlyList<HeartDownloadSource>>())).Returns(true);
        using var rig = new Rig(fallbacks: [spy.Object]);
        rig.Direct.Setup(d => d.DownloadAlbumWithSourceAsync(It.IsAny<string>(), It.IsAny<string>(), It.IsAny<DownloadSource>(),
            It.IsAny<bool>(), It.IsAny<CancellationToken>(), It.IsAny<IReadOnlyList<string>?>())).ReturnsAsync(false);

        await rig.Hearts.AcquireAlbumAsync("soulseek", "album-1", "alice");
        rig.Hearts.QueuePlay("soulseek", "song-9", "alice");

        spy.Verify(f => f.OfferAsync(It.IsAny<ChainMiss>(), It.IsAny<CancellationToken>()), Times.Never);
    }

    [Fact]
    public async Task ASoulseekOutageDuringTheChainIsNotCounted()
    {
        using var rig = new Rig();
        rig.Fallback.Link.State = Octo.Services.Soulseek.SoulseekLinkState.NotLoggedIn;
        for (var i = 0; i < 4; i++)
        {
            await rig.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice");
            rig.Fallback.Now += TimeSpan.FromHours(7);
        }
        Assert.Empty(rig.Fallback.Requests);
        Assert.Equal(0, rig.Fallback.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
    }

    [Fact]
    public async Task TheImportTrickleEntersThroughTheSameDoorAndIsMarkedAsAnImport()
    {
        ChainMiss? seen = null;
        var spy = new Mock<IAcquisitionFallback>();
        spy.Setup(f => f.Applies(It.IsAny<IReadOnlyList<HeartDownloadSource>>())).Returns(true);
        spy.Setup(f => f.OfferAsync(It.IsAny<ChainMiss>(), It.IsAny<CancellationToken>()))
            .Callback<ChainMiss, CancellationToken>((m, _) => seen = m).ReturnsAsync(FallbackResult.Declined);
        using var rig = new Rig(fallbacks: [spy.Object]);

        await rig.Hearts.AcquireTrackAsync("soulseek", "song-1", "alice", origin: MissOrigin.Trickle);

        Assert.Equal(MissOrigin.Trickle, seen!.Origin);
        Assert.Equal(new[] { HeartDownloadSource.Soulseek, HeartDownloadSource.Lidarr }, seen.Steps);
    }
}
