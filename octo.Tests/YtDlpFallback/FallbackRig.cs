using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging.Abstractions;
using Moq;
using Octo.Models.Settings;
using Octo.Services.Common;
using Octo.Services.Lidarr;
using Octo.Services.Soulseek;
using Octo.Services.YtDlpFallback;

namespace Octo.Tests.YtDlpFallback;

/// <summary>A scripted Soulseek: whatever reading the test sets, or none.</summary>
internal sealed class ScriptedLink : ISoulseekLink
{
    public Queue<SoulseekLinkState?> Script { get; } = new();
    public SoulseekLinkState? State { get; set; } = SoulseekLinkState.LoggedIn;
    public int Reads;
    public TimeSpan HoldLimit { get; set; } = TimeSpan.Zero;
    public DateTime UtcNow => DateTime.UtcNow;

    public Task<SoulseekServerReading?> ReadAsync(bool fresh, CancellationToken ct)
    {
        Interlocked.Increment(ref Reads);
        var state = Script.Count > 0 ? Script.Dequeue() : State;
        return Task.FromResult(state is { } s ? new SoulseekServerReading(s, s.ToString(), "u", null) : null);
    }

    public Task<bool> WaitForLoginAsync(DateTime deadlineUtc, CancellationToken ct) => Task.FromResult(true);
}

/// <summary>The fallback with a fake clock, a queue to read and a store in a temporary folder.</summary>
internal sealed class FallbackRig : IDisposable
{
    public static readonly DateTime Start = new(2026, 10, 11, 8, 0, 0, DateTimeKind.Utc);

    public string Dir { get; } = Path.Combine(Path.GetTempPath(), "fla21-" + Guid.NewGuid().ToString("N"));
    public string StorePath => Path.Combine(Dir, "ytdlp-fallback.json");
    public DateTime Now { get; set; } = Start;
    public TestOptionsMonitor<YtDlpFallbackSettings> Settings { get; } = TestOptions.Monitor(new YtDlpFallbackSettings());
    public ScriptedLink Link { get; } = new();
    public TrackAcquisitionQueue Queue { get; } = new(NullLogger<TrackAcquisitionQueue>.Instance);
    public YtDlpFallbackStore Store { get; private set; }
    public YtDlpFallbackService Service { get; private set; }
    public long Free { get; set; } = 100L * 1024 * 1024 * 1024;
    public List<AcquisitionRequest> Requests { get; } = [];
    public string? MarkDuringDownload;
    public bool DownloadFails;

    public FallbackRig(YtDlpFallbackSettings? settings = null, bool withLink = true)
    {
        if (settings is not null) Settings.Set(settings);
        Directory.CreateDirectory(Dir);
        Build(withLink);
        _ = Pump();
    }

    private bool _withLink;

    public void Build(bool withLink = true)
    {
        _withLink = withLink;
        Store = new YtDlpFallbackStore(StorePath, null, () => Now);
        Service = new YtDlpFallbackService(Settings, Store, Queue,
            new ConfigurationBuilder().AddInMemoryCollection(new Dictionary<string, string?> { ["Library:DownloadPath"] = Dir }).Build(),
            NullLogger<YtDlpFallbackService>.Instance, withLink ? Link : null,
            TestOptions.Monitor(new SubsonicSettings()))
        {
            Clock = () => Now,
            FreeBytes = _ => Free,
        };
    }

    // Plays the part of the download worker for the fallback's request.
    private readonly CancellationTokenSource _stop = new();
    private Task Pump() => Task.Run(async () =>
    {
        try
        {
            while (!_stop.IsCancellationRequested)
            {
                var request = await Queue.DequeueAsync(_stop.Token);
                if (request is null) continue;
                Requests.Add(request);
                MarkDuringDownload = AcquisitionMarks.Get(request.ExternalId);
                Queue.Release(request);
                if (DownloadFails) request.Completion.TrySetException(new FileNotFoundException("x"));
                else request.Completion.TrySetResult("/music/song.mp3");
            }
        }
        catch (OperationCanceledException) { }
    });

    public static ChainMiss Miss(string id = "song-1", string? by = "alice", MissOrigin origin = MissOrigin.Heart,
        params HeartDownloadSource[] steps) =>
        new("soulseek", id, by, origin, steps.Length > 0 ? steps : [HeartDownloadSource.Soulseek, HeartDownloadSource.Lidarr]);

    public void Dispose()
    {
        _stop.Cancel();
        try { Directory.Delete(Dir, true); } catch { /* best effort */ }
    }
}
