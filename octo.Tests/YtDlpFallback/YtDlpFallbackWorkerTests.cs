using Microsoft.Extensions.Logging.Abstractions;
using Moq;
using Octo.Models.Settings;
using Octo.Services;
using Octo.Services.Common;
using Octo.Services.Imports;
using Octo.Services.Lidarr;
using Octo.Services.YtDlpFallback;

namespace Octo.Tests.YtDlpFallback;

public sealed class YtDlpFallbackWorkerTests
{
    private sealed class Rig : IDisposable
    {
        public FallbackRig Fallback = new();
        public AcquisitionTracker Tracker = new(NullLogger<AcquisitionTracker>.Instance);
        public Mock<ILidarrHeartAcquisitionService> Lidarr = new();
        public TrackAcquisitionQueue Queue = new(NullLogger<TrackAcquisitionQueue>.Instance);
        public HeartAcquisitionCoordinator Hearts;
        public YtDlpFallbackWorker Worker;
        public TrickleQueue TrickleQueue = new();

        public Rig()
        {
            var settings = new SubsonicSettings
            {
                HeartDownloadSources =
                [
                    new() { Source = HeartDownloadSource.Lidarr, Enabled = true },
                    new() { Source = HeartDownloadSource.Soulseek, Enabled = false },
                    new() { Source = HeartDownloadSource.YouTube, Enabled = false },
                ],
            };
            Hearts = new HeartAcquisitionCoordinator(TestOptions.Monitor(settings), Queue,
                new Mock<IDownloadService>().Object, Lidarr.Object, NullLogger<HeartAcquisitionCoordinator>.Instance,
                tracker: Tracker, fallbacks: [Fallback.Service]);
            Worker = new YtDlpFallbackWorker(Fallback.Settings, Fallback.Store, Hearts,
                NullLogger<YtDlpFallbackWorker>.Instance, Tracker, Fallback.Link, TrickleQueue, Fallback.Service)
            { Clock = () => Fallback.Now };
        }

        public void Put(string id, int counted, string origin = "heart", double firstHoursAgo = 0, double lastCountedHoursAgo = 0)
        {
            var now = Fallback.Now;
            Fallback.Store.Put(new FallbackEntry("soulseek", id, "alice", origin, now.AddHours(-firstHoursAgo), counted,
                counted == 0 ? null : now.AddHours(-lastCountedHoursAgo), now.AddHours(-lastCountedHoursAgo)));
        }

        public void Dispose() => Fallback.Dispose();
    }

    [Fact]
    public async Task ASongDueAgainRunsTheWholeChainAgain()
    {
        using var rig = new Rig();
        rig.Put("song-1", counted: 1, firstHoursAgo: 7, lastCountedHoursAgo: 7);

        Assert.Equal(1, await rig.Worker.TickAsync(default));
        await rig.Worker.DrainAsync();

        rig.Lidarr.Verify(l => l.TryAcquireTrackAsync("soulseek", "song-1", false, "alice"), Times.Once);
        Assert.Equal(2, rig.Fallback.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
    }

    [Fact]
    public async Task ASongNotYetDueIsLeftAlone()
    {
        using var rig = new Rig();
        rig.Put("song-1", counted: 1, firstHoursAgo: 2, lastCountedHoursAgo: 2);
        Assert.Equal(0, await rig.Worker.TickAsync(default));
        rig.Lidarr.VerifyNoOtherCalls();
    }

    [Fact]
    public async Task ASongThatArrivedAnotherWayIsDroppedFromTheCount()
    {
        using var rig = new Rig();
        rig.Put("song-1", counted: 1, firstHoursAgo: 7, lastCountedHoursAgo: 7);
        rig.Lidarr.Setup(l => l.TryAcquireTrackAsync("soulseek", "song-1", false, "alice")).ReturnsAsync(true);

        await rig.Worker.TickAsync(default);
        await rig.Worker.DrainAsync();

        Assert.Null(rig.Fallback.Store.Get("soulseek", "song-1", "alice"));
    }

    [Fact]
    public async Task AfterTheMaximumWaitTheHeartIsGivenUpWithAPlainRowAndACount()
    {
        using var rig = new Rig();
        rig.Tracker.Begin("soulseek", "song-1", null, "alice", "A", "T", null);
        rig.Put("song-1", counted: 2, firstHoursAgo: 73, lastCountedHoursAgo: 10);

        await rig.Worker.TickAsync(default);

        var row = rig.Tracker.All().Single();
        Assert.Equal(AcquisitionState.Failed, row.State);
        Assert.Contains("2 of 3", row.Error);
        Assert.Contains("72 hours", row.Error);
        Assert.Null(rig.Fallback.Store.Get("soulseek", "song-1", "alice"));
        Assert.Equal(1, rig.Fallback.Store.GivenUp.Count);
        rig.Lidarr.VerifyNoOtherCalls();
    }

    [Fact]
    public async Task ASoulseekOutageParkedSongWaitsForSoulseekToBeBack()
    {
        using var rig = new Rig();
        rig.Put("song-1", counted: 0, firstHoursAgo: 1);
        rig.Fallback.Now += TimeSpan.FromMinutes(5);
        rig.Fallback.Link.State = Octo.Services.Soulseek.SoulseekLinkState.NotLoggedIn;
        Assert.Equal(0, await rig.Worker.TickAsync(default));

        rig.Fallback.Link.State = Octo.Services.Soulseek.SoulseekLinkState.LoggedIn;
        Assert.Equal(1, await rig.Worker.TickAsync(default));
        await rig.Worker.DrainAsync();
    }

    [Fact]
    public async Task ImportSongsAreNotRunFromHere()
    {
        using var rig = new Rig();
        rig.Put("song-1", counted: 1, origin: "trickle", firstHoursAgo: 7, lastCountedHoursAgo: 7);
        Assert.Equal(0, await rig.Worker.TickAsync(default));
        rig.Lidarr.VerifyNoOtherCalls();
    }

    [Fact]
    public async Task ImportSongsAreForgottenAfterTheMaximumWaitWithoutARow()
    {
        using var rig = new Rig();
        rig.Put("song-1", counted: 1, origin: "trickle", firstHoursAgo: 80, lastCountedHoursAgo: 80);
        await rig.Worker.TickAsync(default);
        Assert.Null(rig.Fallback.Store.Get("soulseek", "song-1", "alice"));
        Assert.Empty(rig.Tracker.All());
    }

    [Fact]
    public async Task SwitchedOffTheWorkerDoesNothing()
    {
        using var rig = new Rig();
        rig.Put("song-1", counted: 1, firstHoursAgo: 80, lastCountedHoursAgo: 80);
        rig.Fallback.Settings.Set(new YtDlpFallbackSettings { Enabled = false });
        Assert.Equal(0, await rig.Worker.TickAsync(default));
        Assert.NotNull(rig.Fallback.Store.Get("soulseek", "song-1", "alice"));
        rig.Lidarr.VerifyNoOtherCalls();
    }

    [Fact]
    public void ANotFoundImportSongComesBackAfterTheSpacingUntilItsTriesAreUsed_ThenAfterTwoWeeks()
    {
        using var rig = new Rig();
        var job = new TrickleJob { Attempts = 1 };
        Assert.Equal(TimeSpan.FromHours(6), rig.TrickleQueue.RetryAfter!(job));
        job.Attempts = 3;
        Assert.Equal(TimeSpan.FromDays(14), rig.TrickleQueue.RetryAfter!(job));
    }

    [Fact]
    public void ANotFoundImportSongKeepsTheTwoWeekRuleWhenTheFallbackWouldNotAct()
    {
        using var rig = new Rig();
        rig.Fallback.Settings.Set(new YtDlpFallbackSettings { Enabled = false });
        Assert.Equal(TimeSpan.FromDays(14), rig.TrickleQueue.RetryAfter!(new TrickleJob { Attempts = 1 }));
    }

    [Fact]
    public void TheQueueUsesTheRetryRuleWhenItIsAskedWhetherToQueueASongAgain()
    {
        var queue = new TrickleQueue { Clock = () => DateTime.UtcNow };
        var tracks = new[] { new ImportTrack { Key = "k", Title = "T", Artist = "A" } };
        queue.Add("alice", tracks);
        var job = queue.TakeNext()!;
        queue.Update("alice", job.Key, j => j.State = ImportTrackStates.NotFound);
        Assert.Equal(0, queue.Add("alice", tracks));          // two weeks by default

        queue.RetryAfter = _ => TimeSpan.Zero;
        Assert.Equal(1, queue.Add("alice", tracks));
    }
}
