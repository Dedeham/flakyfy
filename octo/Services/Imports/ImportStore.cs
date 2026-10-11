using System.Text.Json;

namespace Octo.Services.Imports;

/// <summary>
/// Every imported list, with its songs and what the last match found, on disk so a restart keeps
/// them and their playlist and fetch switches. Readers get copies, so a list never changes under them.
/// </summary>
public sealed class ImportStore
{
    private sealed class FileShape
    {
        public List<ImportList> Lists { get; set; } = [];
        public Dictionary<string, StarState> Stars { get; set; } = [];
    }

    /// <summary>
    /// The favorites an import still has to set for one person, as Navidrome song ids and nothing
    /// else, and the ids already offered once, so a favorite they took away is not set again.
    /// </summary>
    public sealed class StarState
    {
        public List<string> Pending { get; set; } = [];
        public List<string> Offered { get; set; } = [];
    }

    private readonly string? _path;
    private readonly ILogger<ImportStore>? _logger;
    private readonly object _lock = new();
    private readonly List<ImportList> _lists = [];
    private readonly Dictionary<string, StarState> _stars = new(StringComparer.OrdinalIgnoreCase);

    public ImportStore(string? path = null, ILogger<ImportStore>? logger = null)
    {
        _path = string.IsNullOrWhiteSpace(path) ? null : path;
        _logger = logger;
        try
        {
            if (_path is not null && File.Exists(_path) && JsonSerializer.Deserialize<FileShape>(File.ReadAllText(_path)) is { } shape)
            {
                _lists.AddRange(shape.Lists ?? []);
                foreach (var (owner, state) in shape.Stars ?? []) _stars[owner] = state;
            }
        }
        catch (Exception ex) { _logger?.LogWarning("the imported lists could not be read: {M}", ex.Message); }
    }

    /// <summary>One person's lists, liked songs first, then by name.</summary>
    public IReadOnlyList<ImportList> ListsOf(string owner)
    {
        lock (_lock)
            return _lists.Where(list => Same(list.Owner, owner))
                .OrderBy(list => list.Source == ImportSources.SpotifyLiked ? 0 : 1)
                .ThenBy(list => list.Name, StringComparer.CurrentCultureIgnoreCase)
                .Select(list => list.Copy())
                .ToList();
    }

    /// <summary>Everyone's lists, for the work that runs for all of them.</summary>
    public IReadOnlyList<ImportList> All()
    {
        lock (_lock) return _lists.Select(list => list.Copy()).ToList();
    }

    public ImportList? Get(string owner, string id)
    {
        lock (_lock) return Find(owner, id)?.Copy();
    }

    /// <summary>
    /// Adds a list, or replaces the songs and details of the one with its id. What the person chose
    /// for it (a playlist, fetching, the playlist's id) stays, and so does what the last match found
    /// for each song that is still there, until the next match says otherwise.
    /// </summary>
    public ImportList Save(ImportList read)
    {
        lock (_lock)
        {
            var existing = Find(read.Owner, read.Id);
            if (existing is not null)
            {
                read.KeepPlaylist = existing.KeepPlaylist;
                read.PlaylistId = existing.PlaylistId;
                read.PlaylistNote = existing.PlaylistNote;
                read.GetMissing = existing.GetMissing;
                read.MatchedUtc = existing.MatchedUtc;
                var known = existing.Tracks.GroupBy(track => track.Key).ToDictionary(group => group.Key, group => group.First().LibraryId);
                foreach (var track in read.Tracks)
                    if (track.LibraryId is null && known.TryGetValue(track.Key, out var id)) track.LibraryId = id;
                _lists.Remove(existing);
            }
            _lists.Add(read.Copy());
            Write();
            return read.Copy();
        }
    }

    /// <summary>Changes one list in place. False when there is no such list.</summary>
    public bool Update(string owner, string id, Action<ImportList> change)
    {
        lock (_lock)
        {
            if (Find(owner, id) is not { } list) return false;
            change(list);
            Write();
            return true;
        }
    }

    /// <summary>Changes every list one person has, in one write.</summary>
    public void UpdateAll(string owner, Action<ImportList> change)
    {
        lock (_lock)
        {
            foreach (var list in _lists.Where(list => Same(list.Owner, owner))) change(list);
            Write();
        }
    }

    public bool Remove(string owner, string id)
    {
        lock (_lock)
        {
            if (Find(owner, id) is not { } list) return false;
            _lists.Remove(list);
            Write();
            return true;
        }
    }

    /// <summary>
    /// The songs the library has for this person's liked lists right now. Those not offered before
    /// wait to be favorited; waiting ones the library no longer has (a deleted song, a changed
    /// match) are dropped, so a stale id is never tried again and again.
    /// </summary>
    public void QueueStars(string owner, IReadOnlyCollection<string> current)
    {
        lock (_lock)
        {
            if (!_stars.TryGetValue(owner, out var state))
            {
                if (current.Count == 0) return;
                _stars[owner] = state = new StarState();
            }
            var now = current.ToHashSet(StringComparer.Ordinal);
            var offered = state.Offered.ToHashSet(StringComparer.Ordinal);
            var kept = state.Pending.Where(now.Contains).ToList();
            var queued = kept.ToHashSet(StringComparer.Ordinal);
            kept.AddRange(current.Where(id => !offered.Contains(id) && queued.Add(id)));
            if (kept.SequenceEqual(state.Pending)) return;
            state.Pending = kept;
            Write();
        }
    }

    /// <summary>
    /// Up to this many song ids to favorite now. They count as offered from here: a favorite that
    /// fails is not retried by itself, because the next match queues it again only if it is new.
    /// </summary>
    public IReadOnlyList<string> TakeStars(string owner, int most)
    {
        lock (_lock)
        {
            if (!_stars.TryGetValue(owner, out var state) || state.Pending.Count == 0) return [];
            var taken = state.Pending.Take(most).ToList();
            state.Pending.RemoveRange(0, taken.Count);
            state.Offered.AddRange(taken);
            Write();
            return taken;
        }
    }

    /// <summary>Whether this person has favorites waiting. Cheap enough for every request.</summary>
    public bool HasStars(string owner)
    {
        lock (_lock) return _stars.TryGetValue(owner, out var state) && state.Pending.Count > 0;
    }

    /// <summary>Gives ids back after a favorite failed, so the next request tries them again.</summary>
    public void RequeueStars(string owner, IEnumerable<string> songIds)
    {
        lock (_lock)
        {
            if (!_stars.TryGetValue(owner, out var state)) return;
            var back = songIds.ToHashSet(StringComparer.Ordinal);
            state.Offered.RemoveAll(back.Contains);
            state.Pending.AddRange(back.Where(id => !state.Pending.Contains(id)));
            Write();
        }
    }

    // Called with the lock held.
    private ImportList? Find(string owner, string id) =>
        _lists.FirstOrDefault(list => Same(list.Owner, owner) && list.Id == id);

    private static bool Same(string a, string b) => string.Equals(a, b, StringComparison.OrdinalIgnoreCase);

    // Called with the lock held.
    private void Write()
    {
        if (_path is null) return;
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(_path)!);
            File.WriteAllText(_path + ".tmp", JsonSerializer.Serialize(new FileShape { Lists = _lists, Stars = _stars }));
            File.Move(_path + ".tmp", _path, overwrite: true);
        }
        catch (Exception ex) { _logger?.LogWarning("the imported lists could not be written: {M}", ex.Message); }
    }
}
