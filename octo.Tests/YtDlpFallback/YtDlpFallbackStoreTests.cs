using System.Text.Json;
using Octo.Services.YtDlpFallback;

namespace Octo.Tests.YtDlpFallback;

public sealed class YtDlpFallbackStoreTests : IDisposable
{
    private readonly string _dir = Path.Combine(Path.GetTempPath(), "fla21s-" + Guid.NewGuid().ToString("N"));
    private string Path_ => Path.Combine(_dir, "ytdlp-fallback.json");
    private static readonly DateTime T = new(2026, 10, 11, 8, 0, 0, DateTimeKind.Utc);

    public YtDlpFallbackStoreTests() => Directory.CreateDirectory(_dir);
    public void Dispose() { try { Directory.Delete(_dir, true); } catch { } }

    private static FallbackEntry Entry(string id = "s1", int counted = 2) =>
        new("soulseek", id, "alice", "heart", T, counted, counted == 0 ? null : T, T);

    private YtDlpFallbackStore Store() => new(Path_, null, () => T.AddDays(1));

    [Fact]
    public void AnEntryRoundTrips()
    {
        var store = Store();
        store.Put(Entry());
        var read = Store().Get("soulseek", "s1", "alice");
        Assert.Equal(Entry(), read);
    }

    [Fact]
    public void TheFileAndItsTemporaryCopyAreOwnerOnlyFromTheStart()
    {
        if (OperatingSystem.IsWindows()) return;
        Store().Put(Entry());
        Assert.Equal(UnixFileMode.UserRead | UnixFileMode.UserWrite, File.GetUnixFileMode(Path_));
        Assert.False(File.Exists(Path_ + ".tmp"));
    }

    [Fact]
    public void AMissingFileStartsEmpty() => Assert.Empty(Store().Snapshot());

    [Theory]
    [InlineData("{ not json")]
    [InlineData("[]")]
    [InlineData("")]
    public void AnUnreadableFileStartsEmpty(string text)
    {
        File.WriteAllText(Path_, text);
        Assert.Empty(Store().Snapshot());
    }

    private static string One(Action<Dictionary<string, object?>> change)
    {
        var good = new Dictionary<string, object?>
        {
            ["provider"] = "soulseek", ["externalId"] = "bad", ["requestedBy"] = "alice", ["origin"] = "heart",
            ["firstMissUtc"] = "2026-10-11T08:00:00Z", ["countedAttempts"] = 3,
            ["lastCountedUtc"] = "2026-10-11T08:00:00Z", ["lastRunUtc"] = "2026-10-11T08:00:00Z",
        };
        change(good);
        return JsonSerializer.Serialize(good);
    }

    private static readonly Dictionary<string, Action<Dictionary<string, object?>>> Damage = new()
    {
        ["no provider"] = d => d.Remove("provider"),
        ["no id"] = d => d.Remove("externalId"),
        ["no requester"] = d => d.Remove("requestedBy"),
        ["no origin"] = d => d.Remove("origin"),
        ["unknown origin"] = d => d["origin"] = "something-else",
        ["no first miss"] = d => d.Remove("firstMissUtc"),
        ["unparseable first miss"] = d => d["firstMissUtc"] = "yesterday",
        ["first miss with no zone"] = d => d["firstMissUtc"] = "2026-10-11T08:00:00",
        ["no count"] = d => d.Remove("countedAttempts"),
        ["negative count"] = d => d["countedAttempts"] = -1,
        ["huge count"] = d => d["countedAttempts"] = 100000,
        ["count as text"] = d => d["countedAttempts"] = "three",
        ["no last counted"] = d => d.Remove("lastCountedUtc"),
        ["a count with no time"] = d => d["lastCountedUtc"] = null,
        ["no last run"] = d => d.Remove("lastRunUtc"),
        ["empty id"] = d => d["externalId"] = "",
    };

    public static IEnumerable<object[]> Damaged() => Damage.Keys.Select(k => new object[] { k });

    [Theory]
    [MemberData(nameof(Damaged))]
    public void ADamagedEntryIsSkippedNeverFilledWithDefaults_AndTheOthersAreKept(string damage)
    {
        var good = One(_ => { }).Replace("\"bad\"", "\"good\"");
        File.WriteAllText(Path_, "{\"entries\":[" + One(Damage[damage]) + "," + good + "]}");

        var all = Store().Snapshot();

        Assert.Equal("good", Assert.Single(all).ExternalId);
    }

    [Fact]
    public void ATimeInTheFutureIsReadAsNow()
    {
        File.WriteAllText(Path_, "{\"entries\":[" + One(d =>
        {
            d["firstMissUtc"] = "2030-01-01T00:00:00Z";
            d["lastCountedUtc"] = "2030-01-01T00:00:00Z";
            d["lastRunUtc"] = "2030-01-01T00:00:00Z";
        }) + "]}");
        var entry = Assert.Single(Store().Snapshot());
        Assert.Equal(T.AddDays(1), entry.FirstMissUtc);
        Assert.Equal(T.AddDays(1), entry.LastCountedUtc);
    }

    [Fact]
    public void GivenUpIsOnlyACount()
    {
        var store = Store();
        store.Put(Entry());
        Assert.True(store.Remove(Entry().Key, givenUp: true));
        var again = Store();
        Assert.Equal(1, again.GivenUp.Count);
        Assert.DoesNotContain("s1", File.ReadAllText(Path_));
    }

    [Fact]
    public void NothingIsWrittenUntilSomethingChanges()
    {
        Store();
        Assert.False(File.Exists(Path_));
    }
}
