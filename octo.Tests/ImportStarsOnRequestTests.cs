using System.Collections.Concurrent;
using System.Text;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using Octo.Models.Settings;
using Octo.Services.Common;
using Octo.Services.Imports;
using Octo.Services.Subsonic;

namespace Octo.Tests;

/// <summary>
/// FLA-19: imported liked songs become favorites as the person who imported them, on that person's
/// next verified request, by the existing StarOnArrival path, with no password kept.
/// </summary>
public sealed class ImportStarsOnRequestTests
{
    private readonly ConcurrentQueue<(string Endpoint, Dictionary<string, string> Parameters)> _calls = new();
    private readonly ImportStore _store = new();
    private readonly TestOptionsMonitor<ImportSettings> _settings = TestOptions.Monitor(new ImportSettings());
    private readonly HashSet<string> _alreadyStarred = [];
    private readonly HashSet<string> _refused = [];

    private ImportStarsOnRequest Observer()
    {
        var tracker = new AcquisitionTracker(NullLogger<AcquisitionTracker>.Instance, null);
        var stars = new StarOnArrival(tracker, scopes: null!, TestOptions.Monitor(new SubsonicSettings()), NullLogger<StarOnArrival>.Instance)
        {
            Call = (endpoint, parameters) =>
            {
                _calls.Enqueue((endpoint, parameters));
                var id = parameters.GetValueOrDefault("id", "");
                var body = endpoint == "rest/getSong"
                    ? "{\"subsonic-response\":{\"status\":\"ok\",\"song\":{\"id\":\"" + id + "\""
                      + (_alreadyStarred.Contains(id) ? ",\"starred\":\"2026-10-01T10:00:00Z\"" : "") + "}}}"
                    : _refused.Contains(id) ? """{"subsonic-response":{"status":"failed"}}""" : """{"subsonic-response":{"status":"ok"}}""";
                return Task.FromResult(Encoding.UTF8.GetBytes(body));
            },
        };
        return new ImportStarsOnRequest(_store, stars, _settings, NullLogger<ImportStarsOnRequest>.Instance) { KeepLast = true };
    }

    private static SubsonicCredential Credential(string user, string secret = "t") => SubsonicCredential.From(new Dictionary<string, string>
    {
        ["u"] = user, ["t"] = secret + "-" + user, ["s"] = "salt", ["c"] = "Symfonium",
    })!;

    private List<Dictionary<string, string>> Stars() =>
        _calls.Where(call => call.Endpoint == "rest/star").Select(call => call.Parameters).ToList();

    [Fact]
    public async Task TheNextRequestFavoritesTheWaitingSongsAsThatPersonOnly()
    {
        _store.QueueStars("alice", ["nd-1", "nd-2"]);
        _store.QueueStars("bob", ["nd-9"]);
        var observer = Observer();

        observer.OnVerifiedRequest("alice", Credential("alice"));
        await observer.Last;

        Assert.Equal(["nd-1", "nd-2"], Stars().Select(call => call["id"]));
        Assert.All(Stars(), call => Assert.Equal("alice", call["u"]));
        Assert.True(_store.HasStars("bob"));
        Assert.False(_store.HasStars("alice"));
    }

    [Fact]
    public async Task ASongIsFavoritedOnce_AndALaterRequestHasNothingToDo()
    {
        _store.QueueStars("alice", ["nd-1"]);
        var observer = Observer();
        observer.OnVerifiedRequest("alice", Credential("alice"));
        await observer.Last;
        _calls.Clear();

        observer.OnVerifiedRequest("alice", Credential("alice"));
        await observer.Last;
        // The same song matched again later is not offered again, so a favorite taken away stays gone.
        _store.QueueStars("alice", ["nd-1"]);
        Assert.False(_store.HasStars("alice"));
        Assert.Empty(_calls);
    }

    [Fact]
    public async Task ASongThatIsAlreadyAFavoriteIsNotStarredAgain()
    {
        _alreadyStarred.Add("nd-1");
        _store.QueueStars("alice", ["nd-1"]);
        var observer = Observer();
        observer.OnVerifiedRequest("alice", Credential("alice"));
        await observer.Last;
        Assert.Empty(Stars());
    }

    [Fact]
    public async Task APasswordIsSwappedForATokenBeforeAnyCall()
    {
        _store.QueueStars("alice", ["nd-1"]);
        var observer = Observer();
        var withPassword = SubsonicCredential.From(new Dictionary<string, string> { ["u"] = "alice", ["p"] = "hunter2", ["c"] = "App" })!;
        observer.OnVerifiedRequest("alice", withPassword);
        await observer.Last;
        Assert.NotEmpty(_calls);
        Assert.All(_calls, call =>
        {
            Assert.DoesNotContain("p", call.Parameters.Keys);
            Assert.DoesNotContain("hunter2", call.Parameters.Values);
            Assert.True(call.Parameters.ContainsKey("t") && call.Parameters.ContainsKey("s"));
        });
    }

    [Fact]
    public async Task AFavoriteNavidromeRefusesIsTriedAgainOnTheNextRequest()
    {
        _store.QueueStars("alice", ["nd-1", "nd-2"]);
        _refused.Add("nd-2");
        var observer = Observer();
        observer.OnVerifiedRequest("alice", Credential("alice"));
        await observer.Last;
        Assert.True(_store.HasStars("alice"));
        Assert.Equal(["nd-2"], _store.TakeStars("alice", 10));
    }

    [Fact]
    public async Task ABigListIsFavoritedInBatchesAcrossRequests()
    {
        _store.QueueStars("alice", Enumerable.Range(0, ImportStarsOnRequest.PerRequest + 5).Select(i => $"nd-{i}").ToList());
        var observer = Observer();
        observer.OnVerifiedRequest("alice", Credential("alice"));
        await observer.Last;
        Assert.Equal(ImportStarsOnRequest.PerRequest, Stars().Count);
        observer.OnVerifiedRequest("alice", Credential("alice"));
        await observer.Last;
        Assert.Equal(ImportStarsOnRequest.PerRequest + 5, Stars().Count);
    }

    [Fact]
    public async Task WithTheSwitchOffARequestDoesNothing()
    {
        _store.QueueStars("alice", ["nd-1"]);
        _settings.Set(new ImportSettings { StarLiked = false });
        var observer = Observer();
        observer.OnVerifiedRequest("alice", Credential("alice"));
        await observer.Last;
        Assert.Empty(_calls);
        Assert.True(_store.HasStars("alice"));
    }

    [Fact]
    public void WaitingSongsForTheLibraryNoLongerHasAreDropped()
    {
        _store.QueueStars("alice", ["nd-1", "nd-2"]);
        _store.QueueStars("alice", ["nd-2"]);
        Assert.Equal(["nd-2"], _store.TakeStars("alice", 10));
    }

    [Fact]
    public void WhatWaitsIsOnlySongIds_AndSurvivesARestart()
    {
        var path = Path.Combine(Path.GetTempPath(), $"fla19-{Guid.NewGuid():N}.json");
        try
        {
            new ImportStore(path).QueueStars("alice", ["nd-1"]);
            var written = File.ReadAllText(path);
            Assert.Contains("nd-1", written);
            Assert.DoesNotContain("\"t\"", written);
            Assert.Equal(["nd-1"], new ImportStore(path).TakeStars("alice", 10));
        }
        finally { File.Delete(path); }
    }

    [Fact]
    public void ANotifyReachesEveryObserver_ASlowOrBrokenOneDoesNotStopTheRest_AndAKeyAloneNamesNobody()
    {
        var seen = new List<string>();
        var services = new ServiceCollection()
            .AddSingleton<IVerifiedRequestObserver>(new Recorder(seen, throws: true))
            .AddSingleton<IVerifiedRequestObserver>(new Recorder(seen, throws: false))
            .BuildServiceProvider();

        VerifiedRequests.Notify(services, new Dictionary<string, string> { ["u"] = " alice ", ["t"] = "x", ["s"] = "y" });
        VerifiedRequests.Notify(services, new Dictionary<string, string> { ["apiKey"] = "k" });
        VerifiedRequests.Notify(services, new Dictionary<string, string> { ["u"] = "bob" });

        Assert.Equal(["alice", "alice"], seen);
    }

    [Fact]
    public void ObserversGetTheSignInWithoutThePassword_AndNothingWhenAKeyOrJwtComesWithTheName()
    {
        var seen = new List<IReadOnlyDictionary<string, string>>();
        var services = new ServiceCollection().AddSingleton<IVerifiedRequestObserver>(new Capture(seen)).BuildServiceProvider();

        VerifiedRequests.Notify(services, new Dictionary<string, string> { ["u"] = "alice", ["p"] = "hunter2", ["c"] = "app" });
        VerifiedRequests.Notify(services, new Dictionary<string, string> { ["u"] = "alice", ["t"] = "x", ["s"] = "y", ["apiKey"] = "k" });
        VerifiedRequests.Notify(services, new Dictionary<string, string> { ["u"] = "alice", ["t"] = "x", ["s"] = "y", ["jwt"] = "j" });

        var only = Assert.Single(seen);
        Assert.False(only.ContainsKey("p"));
        Assert.DoesNotContain("hunter2", string.Join(" ", only.Values));
        Assert.True(only.ContainsKey("t") && only.ContainsKey("s"));
    }

    [Fact]
    public void ASongNavidromeKeepsRefusingIsGivenUpOnAfterFiveTries_AndNotQueuedAgain()
    {
        _store.QueueStars("alice", ["nd-1"]);
        for (var attempt = 1; attempt <= ImportStore.MaxStarAttempts; attempt++)
        {
            Assert.Equal(["nd-1"], _store.TakeStars("alice", 10));
            _store.RequeueStars("alice", ["nd-1"]);
        }
        Assert.False(_store.HasStars("alice"));
        _store.QueueStars("alice", ["nd-1"]);
        Assert.False(_store.HasStars("alice"));
    }

    [Fact]
    public void SongsTakenWhenOctoStoppedAreQueuedAgainAtStartup_AndFinishedOnesAreNot()
    {
        var path = Path.Combine(Path.GetTempPath(), $"fla19-{Guid.NewGuid():N}.json");
        try
        {
            var before = new ImportStore(path);
            before.QueueStars("alice", ["nd-1", "nd-2"]);
            before.TakeStars("alice", 10);
            before.DoneStars("alice", ["nd-1"]);   // nd-2 never got an answer: Octo stopped

            var after = new ImportStore(path);
            Assert.Equal(["nd-2"], after.TakeStars("alice", 10));
        }
        finally { File.Delete(path); }
    }

    [Fact]
    public void ASongThatKeepsCrashingOctoIsGivenUpOn()
    {
        var path = Path.Combine(Path.GetTempPath(), $"fla19-{Guid.NewGuid():N}.json");
        try
        {
            new ImportStore(path).QueueStars("alice", ["nd-1"]);
            for (var start = 0; start < ImportStore.MaxStarAttempts; start++)
                new ImportStore(path).TakeStars("alice", 10);   // taken, then the process ends
            Assert.False(new ImportStore(path).HasStars("alice"));
        }
        finally { File.Delete(path); }
    }

    [Fact]
    public void TheImportFileIsReadableByOctoAloneEvenWhenItWasOpenBefore()
    {
        if (OperatingSystem.IsWindows()) return;
        var path = Path.Combine(Path.GetTempPath(), $"fla19-{Guid.NewGuid():N}.json");
        try
        {
            File.WriteAllText(path, "{}");
            File.SetUnixFileMode(path, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.GroupRead | UnixFileMode.OtherRead);
            new ImportStore(path).QueueStars("alice", ["nd-1"]);
            Assert.Equal(UnixFileMode.UserRead | UnixFileMode.UserWrite, File.GetUnixFileMode(path));
            Assert.False(File.Exists(path + ".tmp"));
        }
        finally { File.Delete(path); }
    }

    private sealed class Capture(List<IReadOnlyDictionary<string, string>> seen) : IVerifiedRequestObserver
    {
        public void OnVerifiedRequest(string user, SubsonicCredential credential) => seen.Add(credential.Parameters());
    }

    private sealed class Recorder(List<string> seen, bool throws) : IVerifiedRequestObserver
    {
        public void OnVerifiedRequest(string user, SubsonicCredential credential)
        {
            seen.Add(user);
            if (throws) throw new InvalidOperationException("broken");
        }
    }
}
