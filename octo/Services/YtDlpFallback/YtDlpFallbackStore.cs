using System.Text.Json;

namespace Octo.Services.YtDlpFallback;

/// <summary>One song the chain has failed to get, and how often.</summary>
public sealed record FallbackEntry(string Provider, string ExternalId, string RequestedBy, string Origin,
    DateTime FirstMissUtc, int CountedAttempts, DateTime? LastCountedUtc, DateTime LastRunUtc)
{
    public string Key => YtDlpFallbackStore.KeyOf(Provider, ExternalId, RequestedBy);
}

/// <summary>
/// The songs the fallback is counting, on disk (<c>ytdlp-fallback.json</c>, mode 0600 from the
/// first byte) so a restart keeps the count. It fails toward not fetching: a missing or unreadable
/// file starts everything at zero, and an entry with a missing field, an unknown origin, a count
/// out of range or a bad time is skipped, never filled with defaults. A time in the future is
/// read as now, so a clock that was wrong once cannot shorten a wait later.
/// </summary>
public sealed class YtDlpFallbackStore
{
    internal const int MaxCount = 100;

    private readonly string? _path;
    private readonly ILogger<YtDlpFallbackStore>? _logger;
    private readonly object _lock = new();
    private readonly Dictionary<string, FallbackEntry> _entries = new(StringComparer.Ordinal);
    private int _givenUp;
    private DateTime? _lastGivenUpUtc;

    internal Func<DateTime> Clock { get; set; } = () => DateTime.UtcNow;

    public YtDlpFallbackStore(string? path = null, ILogger<YtDlpFallbackStore>? logger = null, Func<DateTime>? clock = null)
    {
        if (clock is not null) Clock = clock;
        _path = string.IsNullOrWhiteSpace(path) ? null : path;
        _logger = logger;
        Load();
    }

    public static string KeyOf(string provider, string externalId, string requestedBy) =>
        $"{provider.ToLowerInvariant()}|{externalId}|{requestedBy.ToLowerInvariant()}";

    public FallbackEntry? Get(string provider, string externalId, string? requestedBy)
    {
        lock (_lock) return _entries.GetValueOrDefault(KeyOf(provider, externalId, requestedBy ?? ""));
    }

    public IReadOnlyList<FallbackEntry> Snapshot()
    {
        lock (_lock) return _entries.Values.OrderBy(e => e.FirstMissUtc).ToList();
    }

    public int CountFor(string requestedBy)
    {
        lock (_lock) return _entries.Values.Count(e => string.Equals(e.RequestedBy, requestedBy, StringComparison.OrdinalIgnoreCase));
    }

    public int Count { get { lock (_lock) return _entries.Count; } }

    /// <summary>How many songs have been given up on, and when last. Counts only.</summary>
    public (int Count, DateTime? LastUtc) GivenUp { get { lock (_lock) return (_givenUp, _lastGivenUpUtc); } }

    public void Put(FallbackEntry entry)
    {
        lock (_lock)
        {
            _entries[entry.Key] = entry;
            Save();
        }
    }

    public bool Remove(string key, bool givenUp = false)
    {
        lock (_lock)
        {
            if (!_entries.Remove(key)) return false;
            if (givenUp) { _givenUp++; _lastGivenUpUtc = Clock(); }
            Save();
            return true;
        }
    }

    private void Load()
    {
        if (_path is null || !File.Exists(_path)) return;
        try
        {
            using var doc = JsonDocument.Parse(File.ReadAllText(_path));
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object) throw new JsonException("not an object");
            var now = Clock();
            if (root.TryGetProperty("givenUp", out var given) && given.ValueKind == JsonValueKind.Number
                && given.TryGetInt32(out var givenCount) && givenCount >= 0)
                _givenUp = givenCount;
            if (root.TryGetProperty("lastGivenUpUtc", out var lastGiven) && TryTime(lastGiven, now, out var lastGivenAt))
                _lastGivenUpUtc = lastGivenAt;
            if (!root.TryGetProperty("entries", out var list) || list.ValueKind != JsonValueKind.Array) return;
            var skipped = 0;
            foreach (var element in list.EnumerateArray())
            {
                if (TryRead(element, now) is { } entry) _entries[entry.Key] = entry;
                else skipped++;
            }
            if (skipped > 0) _logger?.LogWarning("{Count} damaged entries in the yt-dlp fallback store were skipped", skipped);
        }
        catch (Exception ex)
        {
            _entries.Clear();
            _logger?.LogWarning("The yt-dlp fallback store could not be read ({Type}); every song starts counting again",
                ex.GetType().Name);
        }
    }

    private static string? Str(JsonElement e, string name, bool allowEmpty = false) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String
        && v.GetString() is { } s && s.Length <= 512 && (allowEmpty || s.Length > 0) ? s : null;

    private static bool TryTime(JsonElement v, DateTime now, out DateTime time)
    {
        time = default;
        if (v.ValueKind != JsonValueKind.String
            || !DateTime.TryParse(v.GetString(), null, System.Globalization.DateTimeStyles.RoundtripKind, out var parsed)
            || parsed.Kind != DateTimeKind.Utc) return false;
        time = parsed > now ? now : parsed;
        return true;
    }

    internal static FallbackEntry? TryRead(JsonElement e, DateTime now)
    {
        if (Str(e, "provider") is not { } provider || Str(e, "externalId") is not { } id
            || Str(e, "requestedBy", allowEmpty: true) is not { } by || Str(e, "origin") is not { } origin
            || origin is not ("heart" or "trickle")) return null;
        if (!e.TryGetProperty("firstMissUtc", out var first) || !TryTime(first, now, out var firstAt)) return null;
        if (!e.TryGetProperty("lastRunUtc", out var run) || !TryTime(run, now, out var runAt)) return null;
        if (!e.TryGetProperty("countedAttempts", out var count) || count.ValueKind != JsonValueKind.Number
            || !count.TryGetInt32(out var counted) || counted < 0 || counted > MaxCount) return null;
        if (!e.TryGetProperty("lastCountedUtc", out var last)) return null;
        DateTime? lastAt = null;
        if (last.ValueKind == JsonValueKind.Null)
        {
            if (counted != 0) return null;
        }
        else if (TryTime(last, now, out var parsedLast)) lastAt = parsedLast;
        else return null;
        if (counted > 0 && lastAt is null) return null;
        return new FallbackEntry(provider, id, by, origin, firstAt, counted, lastAt, runAt);
    }

    // Called with the lock held. A temporary file created 0600, then moved over the target.
    private void Save()
    {
        if (_path is null) return;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(_path))!);
            var temp = _path + ".tmp";
            var options = new FileStreamOptions
            {
                Mode = FileMode.Create, Access = FileAccess.Write, Share = FileShare.None,
            };
            if (!OperatingSystem.IsWindows())
                options.UnixCreateMode = UnixFileMode.UserRead | UnixFileMode.UserWrite;
            using (var stream = new FileStream(temp, options))
            {
                JsonSerializer.Serialize(stream, new
                {
                    givenUp = _givenUp,
                    lastGivenUpUtc = _lastGivenUpUtc,
                    entries = _entries.Values.Select(e => new
                    {
                        provider = e.Provider, externalId = e.ExternalId, requestedBy = e.RequestedBy, origin = e.Origin,
                        firstMissUtc = e.FirstMissUtc, countedAttempts = e.CountedAttempts,
                        lastCountedUtc = e.LastCountedUtc, lastRunUtc = e.LastRunUtc,
                    }),
                });
            }
            File.Move(temp, _path, overwrite: true);
        }
        catch (Exception ex) { _logger?.LogWarning("The yt-dlp fallback store could not be written ({Type})", ex.GetType().Name); }
    }
}
