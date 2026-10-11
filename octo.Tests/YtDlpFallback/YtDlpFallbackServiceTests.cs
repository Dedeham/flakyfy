using Octo.Models.Settings;
using Octo.Services.Common;
using Octo.Services.Soulseek;
using Octo.Services.YtDlpFallback;

namespace Octo.Tests.YtDlpFallback;

public sealed class YtDlpFallbackServiceTests
{
    private static async Task<FallbackResult> Run(FallbackRig rig, ChainMiss? miss = null) =>
        await rig.Service.OfferAsync(miss ?? FallbackRig.Miss());

    private static void Wait(FallbackRig rig, double hours) => rig.Now += TimeSpan.FromHours(hours);

    [Fact]
    public async Task ThreeCountedRunsSixHoursApartAndThenYouTubeAsAnMp3()
    {
        using var rig = new FallbackRig();
        Assert.Equal(FallbackResult.Parked, await Run(rig));
        Wait(rig, 6);
        Assert.Equal(FallbackResult.Parked, await Run(rig));
        Assert.Empty(rig.Requests);
        Wait(rig, 6);

        Assert.Equal(FallbackResult.Obtained, await Run(rig));

        var request = Assert.Single(rig.Requests);
        Assert.Equal(DownloadSource.YouTube, request.SourceOverride);
        Assert.Equal("song-1", request.ExternalId);
        Assert.Null(rig.Store.Get("soulseek", "song-1", "alice")); // done: nothing left to count
    }

    [Fact]
    public async Task RunsCloserThanTheSpacingAreNotCountedTwice()
    {
        using var rig = new FallbackRig();
        await Run(rig);
        Wait(rig, 1);
        await Run(rig);
        Wait(rig, 1);
        await Run(rig);
        Wait(rig, 1);
        await Run(rig);

        Assert.Equal(1, rig.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
        Assert.Empty(rig.Requests);
    }

    [Fact]
    public async Task AHeartedAgainSongNeverStartsOver()
    {
        using var rig = new FallbackRig();
        await Run(rig);
        Wait(rig, 6);
        await Run(rig);
        // Heart, unheart, heart again in quick succession: the count and the first miss stay.
        await Run(rig);
        await Run(rig);
        var entry = rig.Store.Get("soulseek", "song-1", "alice")!;
        Assert.Equal(2, entry.CountedAttempts);
        Assert.Equal(FallbackRig.Start, entry.FirstMissUtc);
    }

    [Theory]
    [InlineData(SoulseekLinkState.NotLoggedIn)]
    [InlineData(SoulseekLinkState.Unknown)]
    [InlineData(null)]
    public async Task ASoulseekOutageNeverCounts_NotEvenAnUnclearOne(SoulseekLinkState? state)
    {
        using var rig = new FallbackRig();
        rig.Link.State = state;
        for (var i = 0; i < 5; i++)
        {
            Assert.Equal(FallbackResult.Parked, await Run(rig));
            Wait(rig, 7);
        }
        Assert.Equal(0, rig.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
        Assert.Empty(rig.Requests);
    }

    [Fact]
    public async Task WithoutALinkAtAllSoulseekCountsAsDown()
    {
        using var rig = new FallbackRig(withLink: false);
        for (var i = 0; i < 4; i++) { await Run(rig); Wait(rig, 7); }
        Assert.Empty(rig.Requests);
        Assert.Equal(0, rig.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
    }

    [Fact]
    public async Task ALidarrFailureCounts_SoLidarrTroubleCannotBlockTheFallback()
    {
        using var rig = new FallbackRig();
        var miss = FallbackRig.Miss(steps: [HeartDownloadSource.Soulseek, HeartDownloadSource.Lidarr]);
        for (var i = 0; i < 3; i++) { await Run(rig, miss); Wait(rig, 6); }
        Assert.Single(rig.Requests);
    }

    [Fact]
    public async Task SoulseekGoingDownAfterTheCountIsReachedWinsOverTheDownload()
    {
        using var rig = new FallbackRig();
        await Run(rig); Wait(rig, 6);
        await Run(rig); Wait(rig, 6);
        // The third run reads Soulseek up for the count, then down just before the download.
        rig.Link.Script.Enqueue(SoulseekLinkState.LoggedIn);
        rig.Link.Script.Enqueue(SoulseekLinkState.NotLoggedIn);

        Assert.Equal(FallbackResult.Parked, await Run(rig));

        Assert.Empty(rig.Requests);
        Assert.Equal(3, rig.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
    }

    [Fact]
    public async Task WithoutSoulseekInTheChainThereIsNothingToWaitFor()
    {
        using var rig = new FallbackRig();
        rig.Link.State = SoulseekLinkState.NotLoggedIn;
        var miss = FallbackRig.Miss(steps: [HeartDownloadSource.Lidarr]);
        for (var i = 0; i < 3; i++) { await Run(rig, miss); Wait(rig, 6); }
        Assert.Single(rig.Requests);
    }

    [Fact]
    public async Task YouTubeInThePersonsOwnChainMeansNothingHappens()
    {
        using var rig = new FallbackRig();
        var miss = FallbackRig.Miss(steps: [HeartDownloadSource.Soulseek, HeartDownloadSource.YouTube]);
        for (var i = 0; i < 4; i++) { Assert.Equal(FallbackResult.Declined, await Run(rig, miss)); Wait(rig, 7); }
        Assert.Empty(rig.Requests);
        Assert.False(File.Exists(rig.StorePath));
        Assert.False(rig.Service.Applies([HeartDownloadSource.Soulseek, HeartDownloadSource.YouTube]));
    }

    [Fact]
    public async Task SwitchedOffMeansNoCallNoFileNoRead()
    {
        using var rig = new FallbackRig(new YtDlpFallbackSettings { Enabled = false });
        for (var i = 0; i < 4; i++) { Assert.Equal(FallbackResult.Declined, await Run(rig)); Wait(rig, 7); }
        Assert.Empty(rig.Requests);
        Assert.False(File.Exists(rig.StorePath));
        Assert.Equal(0, rig.Link.Reads);
        Assert.False(rig.Service.Applies([HeartDownloadSource.Soulseek]));
    }

    [Fact]
    public async Task TheSwitchIsReadWhenItRuns()
    {
        using var rig = new FallbackRig();
        Assert.True(rig.Service.Applies([HeartDownloadSource.Soulseek]));
        rig.Settings.Set(new YtDlpFallbackSettings { Enabled = false });
        Assert.False(rig.Service.Applies([HeartDownloadSource.Soulseek]));
        Assert.Equal(FallbackResult.Declined, await Run(rig));
    }

    [Fact]
    public async Task AnImportSongIsDeclinedUntilItsLastCountedTry_ThenFetched()
    {
        using var rig = new FallbackRig();
        var miss = FallbackRig.Miss(origin: MissOrigin.Trickle);
        Assert.Equal(FallbackResult.Declined, await Run(rig, miss)); Wait(rig, 6);
        Assert.Equal(FallbackResult.Declined, await Run(rig, miss)); Wait(rig, 6);
        Assert.Equal(FallbackResult.Obtained, await Run(rig, miss));
        Assert.Single(rig.Requests);
    }

    [Fact]
    public async Task TheDownloadCarriesTheFallbackMarkAndOnlyWhileItRuns()
    {
        using var rig = new FallbackRig(new YtDlpFallbackSettings { FailedAttempts = 2 });
        await Run(rig); Wait(rig, 6);
        await Run(rig);
        Assert.Equal(AcquisitionMarks.YouTubeFallback, rig.MarkDuringDownload);
        Assert.Null(AcquisitionMarks.Get("song-1"));
    }

    [Fact]
    public async Task AFailedDownloadWaitsForAHeartAndKeepsTheCount()
    {
        using var rig = new FallbackRig(new YtDlpFallbackSettings { FailedAttempts = 2 });
        rig.DownloadFails = true;
        await Run(rig); Wait(rig, 6);
        Assert.Equal(FallbackResult.Parked, await Run(rig));
        Assert.Equal(2, rig.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
        Assert.Null(AcquisitionMarks.Get("song-1"));
    }

    [Fact]
    public async Task NoRoomOnTheDiskMeansNoDownload()
    {
        using var rig = new FallbackRig(new YtDlpFallbackSettings { FailedAttempts = 2 });
        rig.Free = 1024;
        await Run(rig); Wait(rig, 6);
        Assert.Equal(FallbackResult.Parked, await Run(rig));
        Assert.Empty(rig.Requests);
    }

    [Fact]
    public async Task ASpaceReadingThatFailsIsTreatedAsNoRoom()
    {
        using var rig = new FallbackRig(new YtDlpFallbackSettings { FailedAttempts = 2 });
        rig.Service.FreeBytes = _ => throw new IOException("no");
        await Run(rig); Wait(rig, 6);
        Assert.Equal(FallbackResult.Parked, await Run(rig));
        Assert.Empty(rig.Requests);
    }

    [Fact]
    public async Task OnePersonCannotUseUpTheHourForEveryone()
    {
        using var rig = new FallbackRig(new YtDlpFallbackSettings { FailedAttempts = 2 });
        for (var i = 0; i < 6; i++) await Run(rig, FallbackRig.Miss($"a-{i}", "alice"));
        Wait(rig, 6);
        for (var i = 0; i < 6; i++) await Run(rig, FallbackRig.Miss($"a-{i}", "alice"));
        Assert.Equal(YtDlpFallbackSettings.PerUserPerHour, rig.Requests.Count);

        // Bob still has his own share of the hour.
        for (var i = 0; i < 2; i++) await Run(rig, FallbackRig.Miss($"b-{i}", "bob"));
        Wait(rig, 6);
        Assert.Equal(FallbackResult.Obtained, await Run(rig, FallbackRig.Miss("b-0", "bob")));
    }

    [Fact]
    public async Task APersonsPileIsCapped()
    {
        using var rig = new FallbackRig();
        for (var i = 0; i < YtDlpFallbackSettings.MaxEntriesPerUser; i++)
            await Run(rig, FallbackRig.Miss($"s-{i}", "alice"));
        Assert.Equal(FallbackResult.Declined, await Run(rig, FallbackRig.Miss("one-more", "alice")));
        Assert.Equal(FallbackResult.Parked, await Run(rig, FallbackRig.Miss("fine", "bob")));
    }

    [Fact]
    public async Task TwoPeopleHeartingTheSameSongCountSeparately()
    {
        using var rig = new FallbackRig();
        await Run(rig, FallbackRig.Miss("song-1", "alice"));
        Wait(rig, 6);
        await Run(rig, FallbackRig.Miss("song-1", "bob"));
        Assert.Equal(1, rig.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
        Assert.Equal(1, rig.Store.Get("soulseek", "song-1", "bob")!.CountedAttempts);
    }

    private static ChainMiss RunStartedAt(DateTime started, string by = "alice") =>
        new("soulseek", "song-1", by, MissOrigin.Heart,
            [HeartDownloadSource.Soulseek, HeartDownloadSource.Lidarr], started);

    [Fact]
    public async Task ARunThatWasGoingWhenTheHeartWasTakenBackLeavesNoEntry()
    {
        using var rig = new FallbackRig();
        var started = rig.Now;
        Wait(rig, 0.01);
        rig.Service.Forget("soulseek", "song-1", "alice");

        Assert.Equal(FallbackResult.Declined, await rig.Service.OfferAsync(RunStartedAt(started)));

        Assert.Null(rig.Store.Get("soulseek", "song-1", "alice"));
        Assert.False(File.Exists(rig.StorePath));
    }

    [Fact]
    public async Task ARunThatWasGoingAfterATakenBackHeartNeverCountsEvenForAnEntryThatExisted()
    {
        using var rig = new FallbackRig();
        await Run(rig);
        var started = rig.Now;
        Wait(rig, 1);
        rig.Service.Forget("soulseek", "song-1", "alice");
        Assert.Null(rig.Store.Get("soulseek", "song-1", "alice"));

        await rig.Service.OfferAsync(RunStartedAt(started));

        Assert.Null(rig.Store.Get("soulseek", "song-1", "alice"));
    }

    [Fact]
    public async Task AReHeartAfterTakingItBackCountsAgainFromOne()
    {
        using var rig = new FallbackRig();
        rig.Service.Forget("soulseek", "song-1", "alice");
        Wait(rig, 1);

        Assert.Equal(FallbackResult.Parked, await rig.Service.OfferAsync(RunStartedAt(rig.Now)));

        Assert.Equal(1, rig.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
        // And the mark is gone: a later run is not mistaken for an old one.
        Wait(rig, 6);
        await rig.Service.OfferAsync(RunStartedAt(rig.Now));
        Assert.Equal(2, rig.Store.Get("soulseek", "song-1", "alice")!.CountedAttempts);
    }

    [Fact]
    public async Task TakingBackOnePersonsHeartDoesNotStopAnotherPersonsRun()
    {
        using var rig = new FallbackRig();
        var started = rig.Now;
        Wait(rig, 0.01);
        rig.Service.Forget("soulseek", "song-1", "alice");
        await rig.Service.OfferAsync(RunStartedAt(started, "bob"));
        Assert.NotNull(rig.Store.Get("soulseek", "song-1", "bob"));
    }

    [Fact]
    public async Task WithoutARecordedRequesterOneUnstarDropsTheSharedEntry()
    {
        using var rig = new FallbackRig();
        await rig.Service.OfferAsync(RunStartedAt(rig.Now, ""));
        Assert.NotNull(rig.Store.Get("soulseek", "song-1", ""));
        rig.Service.Forget("soulseek", "song-1", null);
        Assert.Null(rig.Store.Get("soulseek", "song-1", ""));
    }

    [Fact]
    public async Task TheCountSurvivesARestart()
    {
        using var rig = new FallbackRig();
        await Run(rig); Wait(rig, 6);
        await Run(rig);
        rig.Build();

        Wait(rig, 6);
        Assert.Equal(FallbackResult.Obtained, await Run(rig));
    }

    [Fact]
    public async Task ADamagedStoreFailsTowardNotFetching()
    {
        using var rig = new FallbackRig();
        File.WriteAllText(rig.StorePath, "{ not json");
        rig.Build();
        Assert.Equal(FallbackResult.Parked, await Run(rig));
        Wait(rig, 6);
        Assert.Equal(FallbackResult.Parked, await Run(rig));
        Assert.Empty(rig.Requests);
    }

    [Fact]
    public async Task LogsNeverCarrySongNames()
    {
        // The service logs counts and exception types only; nothing it logs takes the miss's ids.
        var logger = new CapturingLogger<YtDlpFallbackService>();
        using var rig = new FallbackRig(new YtDlpFallbackSettings { FailedAttempts = 2 });
        rig.DownloadFails = true;
        var service = new YtDlpFallbackService(rig.Settings, rig.Store, rig.Queue,
            new Microsoft.Extensions.Configuration.ConfigurationBuilder().Build(), logger, rig.Link)
        { Clock = () => rig.Now, FreeBytes = _ => rig.Free };
        var miss = FallbackRig.Miss("secret-id", "secret-user");
        await service.OfferAsync(miss); rig.Now += TimeSpan.FromHours(6);
        await service.OfferAsync(miss);
        Assert.NotEmpty(logger.Lines);
        Assert.All(logger.Lines, l => { Assert.DoesNotContain("secret-", l); });
    }
}

internal sealed class CapturingLogger<T> : Microsoft.Extensions.Logging.ILogger<T>
{
    public List<string> Lines { get; } = [];
    public IDisposable? BeginScope<TState>(TState state) where TState : notnull => null;
    public bool IsEnabled(Microsoft.Extensions.Logging.LogLevel logLevel) => true;
    public void Log<TState>(Microsoft.Extensions.Logging.LogLevel logLevel, Microsoft.Extensions.Logging.EventId eventId,
        TState state, Exception? exception, Func<TState, Exception?, string> formatter) =>
        Lines.Add(formatter(state, exception) + " " + exception);
}
