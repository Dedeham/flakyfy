using System.Text.Json;
using Microsoft.Extensions.Options;
using Octo.Models.Settings;

namespace Octo.Services.Library;

/// <summary>A move into or out of the trash. A restore answers the outside-song mappings the
/// removal dropped, for the library to have again.</summary>
public sealed record QuarantineResult(bool Moved, string? QuarantinePath, string? Error,
    IReadOnlyList<Octo.Services.Local.LocalSongMapping>? Mappings = null);

/// <summary>A file of the song's own that went to the trash with it: where it was, and where it waits.</summary>
public sealed record QuarantinedSidecar(string OriginalPath, string QuarantinePath);

/// <summary>What is written next to a quarantined file so a restore works without the journal.
/// Sidecars are the song's own files that went with it (its lyrics), put back with it; Mappings
/// the outside songs Octo knew the file as, which a removal forgets and a restore puts back.</summary>
public sealed record QuarantineManifest(
    string OriginalPath, string NavidromeId, string Action, string Username, DateTime AtUtc,
    IReadOnlyList<QuarantinedSidecar>? Sidecars = null, IReadOnlyList<Octo.Services.Local.LocalSongMapping>? Mappings = null);

/// <summary>
/// Moves a verified file out of the library instead of deleting it.
///
/// There is no File.Delete anywhere in library actions except the retention sweep. These
/// actions delete files Octo did NOT create, on one tap in a music client with no confirmation
/// dialog, and the whole point of the feature is that the user is correcting a mistake, which
/// means they can make one.
///
/// This is where DiscardRejectedDownload's philosophy is reconciled rather than contradicted.
/// That guard refuses to delete anything created before the attempt started, because its input
/// is a GUESS: ResolveLocalPath matches on leaf name and approximate size. Here the user has
/// pointed at a specific track, so a creation-time guard would break the feature's purpose. The
/// invariant underneath still holds: never act on a path you inferred rather than proved. That
/// is discharged by the resolver's byte-size check, which is strictly stronger than
/// DiscardRejectedDownload's own approximate-size matching, plus quarantine instead of delete,
/// which makes getting it wrong recoverable rather than terminal.
/// </summary>
public sealed class LibraryActionQuarantine
{
    private const string ManifestSuffix = ".octo-action.json";

    private readonly IOptionsMonitor<LibraryActionSettings> _settings;
    private readonly ILogger<LibraryActionQuarantine> _logger;

    public LibraryActionQuarantine(IOptionsMonitor<LibraryActionSettings> settings,
        ILogger<LibraryActionQuarantine> logger)
    {
        _settings = settings;
        _logger = logger;
    }

    public string RootFor(string musicRoot) =>
        Path.Combine(musicRoot, _settings.CurrentValue.EffectiveQuarantineDirectory);

    /// <summary>
    /// Files beside a song that belong to it alone, by its name: the lyrics Octo writes (.lrc,
    /// .txt) and the other lyrics files players read. Never a folder's cover, which the album shares.
    /// </summary>
    internal static readonly string[] SidecarExtensions = [".lrc", ".txt", ".ttml", ".elrc", ".srt", ".yaml", ".yml"];

    private static readonly HashSet<string> AudioExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".flac", ".mp3", ".m4a", ".aac", ".ogg", ".oga", ".opus", ".wav", ".aiff", ".aif", ".ape", ".wv", ".alac",
        ".wma", ".mp4", ".dsf", ".dff", ".mka",
    };

    private static StringComparison NameComparison =>
        OperatingSystem.IsWindows() ? StringComparison.OrdinalIgnoreCase : StringComparison.Ordinal;

    /// <summary>
    /// The song's own sidecars: files with its name and a lyrics extension. None while another
    /// audio file of the same name stays beside it (a second copy in another format), since the
    /// lyrics are that copy's too.
    /// </summary>
    internal static IReadOnlyList<string> SidecarsOf(string audioPath)
    {
        var folder = Path.GetDirectoryName(audioPath);
        if (string.IsNullOrEmpty(folder) || !Directory.Exists(folder)) return [];
        var stem = Path.GetFileNameWithoutExtension(audioPath);
        var named = Directory.EnumerateFiles(folder)
            .Where(path => string.Equals(Path.GetFileNameWithoutExtension(path), stem, NameComparison))
            .ToList();
        if (named.Any(path => !string.Equals(path, audioPath, NameComparison) && AudioExtensions.Contains(Path.GetExtension(path))))
            return [];
        return named
            .Where(path => SidecarExtensions.Contains(Path.GetExtension(path), StringComparer.OrdinalIgnoreCase))
            .ToList();
    }

    /// <summary>
    /// Move a verified file into quarantine, preserving its layout underneath so a restore is a
    /// straight copy back. With <paramref name="withSidecars"/>, the song's own lyrics files go
    /// with it, named after its place in the trash; one that cannot move stays, and is logged.
    /// </summary>
    public QuarantineResult Move(ResolvedSongFile file, string musicRoot, LibraryAction action, string username,
        bool withSidecars = false)
    {
        try
        {
            if (Outside(file.AbsolutePath, musicRoot) is { } refused) return new QuarantineResult(false, null, refused);
            var relative = Path.GetRelativePath(musicRoot, file.AbsolutePath);
            // Read before the song moves: with it gone, nothing would tell its lyrics from another copy's.
            var sidecars = withSidecars ? SidecarsOf(file.AbsolutePath) : [];

            var destination = Path.Combine(RootFor(musicRoot),
                DateTime.UtcNow.ToString("yyyy-MM-dd"), relative);
            // Nothing is created while a folder on the way to the trash is a link.
            if (LinkBelow(musicRoot, destination) is { } trashLink)
                return new QuarantineResult(false, null, $"the trash path goes through a link ({trashLink})");
            Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
            destination = Unique(destination);

            // The checks above ran before folders were created; run them again with nothing
            // between them and the move.
            if (Outside(file.AbsolutePath, musicRoot) is { } refusedNow) return new QuarantineResult(false, null, refusedNow);
            if (LinkBelow(musicRoot, destination) is { } trashLinkNow)
                return new QuarantineResult(false, null, $"the trash path goes through a link ({trashLinkNow})");

            try
            {
                File.Move(file.AbsolutePath, destination);
            }
            catch (IOException)
            {
                // Across devices Move fails, so copy, confirm the copy is identical, then remove.
                // The comparison is what stops a damaged copy from turning into a delete.
                if (!CopyVerified(file.AbsolutePath, destination, file.SizeBytes))
                    return new QuarantineResult(false, null, "the copy did not match the original");
                File.Delete(file.AbsolutePath);
            }

            var moved = new List<QuarantinedSidecar>();
            foreach (var sidecar in sidecars)
            {
                var target = Path.Combine(Path.GetDirectoryName(destination)!,
                    Path.GetFileNameWithoutExtension(destination) + Path.GetExtension(sidecar));
                if (MoveSidecar(sidecar, target, musicRoot)) moved.Add(new QuarantinedSidecar(sidecar, target));
            }

            WriteManifest(destination, new QuarantineManifest(
                file.AbsolutePath, file.NavidromeId, action.ToString(), username, DateTime.UtcNow,
                moved.Count == 0 ? null : moved));

            _logger.LogInformation("Library action {Action} by {User}: quarantined {From} -> {To}",
                action, username, file.AbsolutePath, destination);
            return new QuarantineResult(true, destination, null);
        }
        catch (Exception ex)
        {
            _logger.LogWarning("Library action could not quarantine {Path}: {M}", file.AbsolutePath, ex.Message);
            return new QuarantineResult(false, null, ex.Message);
        }
    }

    /// <summary>
    /// Why a file may not be moved out of the library, or null when it may. The resolver proved
    /// it already; this is the last word before a move, so it does not trust that: the full path
    /// must sit inside the music root (a "..", another drive or a lookalike folder name such as
    /// "/music-old" all fail), must not already be in the trash, and must be a real file rather
    /// than a link that could point anywhere.
    /// </summary>
    internal string? Outside(string path, string musicRoot)
    {
        string full;
        try { full = Path.GetFullPath(path); }
        catch (Exception) { return "the file's path cannot be read"; }
        if (!NavidromeSongPathResolver.IsInside(full, musicRoot)) return "the file is not under the music root";
        if (NavidromeSongPathResolver.IsInside(full, RootFor(musicRoot))) return "the file is in the trash already";
        var info = new FileInfo(full);
        if (info.Exists && info.Attributes.HasFlag(FileAttributes.ReparsePoint)) return "the file is a link, not a song file";
        if (Directory.Exists(full)) return "the path is a folder, not a song file";
        if (LinkBelow(musicRoot, full) is { } link) return $"a folder on the way to the file is a link ({link})";
        return null;
    }

    /// <summary>
    /// The first folder between the music root and <paramref name="path"/> that is a link, or
    /// null when there is none. The root itself may be a link (it is the one place the library
    /// is mounted), so it is the start: with no link below it, the real path of the file is the
    /// root's real path plus the same relative path. Folders that do not exist yet cannot be
    /// links. A path outside the root answers with itself.
    /// </summary>
    internal static string? LinkBelow(string musicRoot, string path)
    {
        var root = Path.GetFullPath(musicRoot).TrimEnd(Path.DirectorySeparatorChar);
        var full = Path.GetFullPath(path);
        if (!NavidromeSongPathResolver.IsInside(full, root)) return full;
        for (var dir = Path.GetDirectoryName(full); dir is not null && dir.Length > root.Length;
             dir = Path.GetDirectoryName(dir))
        {
            var info = new DirectoryInfo(dir);
            if (info.Exists && (info.LinkTarget is not null || info.Attributes.HasFlag(FileAttributes.ReparsePoint)))
                return dir;
        }
        return null;
    }

    /// <summary>Copy, then compare length and SHA-256 with the original. A copy that differs
    /// is removed and false is returned; the original is never touched here.</summary>
    private static bool CopyVerified(string from, string to, long expectedLength)
    {
        try
        {
            File.Copy(from, to, overwrite: false);
            if (new FileInfo(to).Length == expectedLength && new FileInfo(from).Length == expectedLength
                && HashOf(from).AsSpan().SequenceEqual(HashOf(to)))
                return true;
        }
        catch { /* an unreadable copy is a bad copy */ }
        TryDelete(to);
        return false;
    }

    private static byte[] HashOf(string path)
    {
        using var stream = File.OpenRead(path);
        return System.Security.Cryptography.SHA256.HashData(stream);
    }

    /// <summary>Put a quarantined file back where it came from. With <paramref name="musicRoot"/>,
    /// only into it, whatever the manifest beside the file says.</summary>
    public QuarantineResult Restore(string quarantinePath, string? musicRoot = null)
    {
        try
        {
            if (!File.Exists(quarantinePath))
                return new QuarantineResult(false, null, "the quarantined file is gone");

            var manifest = ReadManifest(quarantinePath);
            if (manifest is null)
                return new QuarantineResult(false, null, "no manifest, so the original path is unknown");
            // The manifest is only a note beside the file, so the root it must stay inside comes
            // from the caller, or else from where the trash sits (the folder above the trash).
            musicRoot ??= RootOfTrashedFile(quarantinePath);
            if (musicRoot is null)
                return new QuarantineResult(false, null, "the file is not in the trash");
            if (!NavidromeSongPathResolver.IsInside(Path.GetFullPath(manifest.OriginalPath), musicRoot))
                return new QuarantineResult(false, null, "its old place is not under the music root");
            if (RestoreRefusal(quarantinePath, manifest.OriginalPath, musicRoot) is { } blocked)
                return new QuarantineResult(false, null, blocked);

            Directory.CreateDirectory(Path.GetDirectoryName(manifest.OriginalPath)!);
            if (File.Exists(manifest.OriginalPath))
                return new QuarantineResult(false, null, "something already exists at the original path");

            // Again with nothing between the check and the move.
            if (RestoreRefusal(quarantinePath, manifest.OriginalPath, musicRoot) is { } blockedNow)
                return new QuarantineResult(false, null, blockedNow);
            File.Move(quarantinePath, manifest.OriginalPath);
            foreach (var sidecar in manifest.Sidecars ?? [])
            {
                // Each one meets the same rule as the song: back only into the music folder.
                if (!NavidromeSongPathResolver.IsInside(Path.GetFullPath(sidecar.OriginalPath), musicRoot))
                    continue;
                if (!File.Exists(sidecar.QuarantinePath) || File.Exists(sidecar.OriginalPath)) continue;
                if (RestoreRefusal(sidecar.QuarantinePath, sidecar.OriginalPath, musicRoot) is not null) continue;
                try { File.Move(sidecar.QuarantinePath, sidecar.OriginalPath); }
                catch (Exception ex) { _logger.LogWarning("Library action could not put back {Path}: {M}", sidecar.OriginalPath, ex.Message); }
            }
            TryDelete(quarantinePath + ManifestSuffix);

            _logger.LogInformation("Library action restored {Path}", manifest.OriginalPath);
            return new QuarantineResult(true, manifest.OriginalPath, null, manifest.Mappings);
        }
        catch (Exception ex)
        {
            _logger.LogWarning("Library action could not restore {Path}: {M}", quarantinePath, ex.Message);
            return new QuarantineResult(false, null, ex.Message);
        }
    }

    /// <summary>Why a trashed file may not go back to <paramref name="original"/>, or null: both
    /// ends must sit under the music root with no link in any folder on the way, the trash side
    /// included, and the old place must not be a folder.</summary>
    private string? RestoreRefusal(string quarantinePath, string original, string musicRoot)
    {
        if (!NavidromeSongPathResolver.IsInside(Path.GetFullPath(quarantinePath), RootFor(musicRoot)))
            return "the file is not in the trash";
        if (LinkBelow(musicRoot, quarantinePath) is { } trashLink)
            return $"the trash path goes through a link ({trashLink})";
        if (new FileInfo(quarantinePath).Attributes.HasFlag(FileAttributes.ReparsePoint))
            return "the trashed file is a link, not a song file";
        if (LinkBelow(musicRoot, original) is { } link)
            return $"a folder on the way to its old place is a link ({link})";
        if (Directory.Exists(original)) return "its old place is a folder";
        return null;
    }

    /// <summary>The music root a trashed file sits under: the folder holding the trash folder. Null
    /// when the path is not under a trash folder of this name.</summary>
    private string? RootOfTrashedFile(string quarantinePath)
    {
        var marker = Path.DirectorySeparatorChar + _settings.CurrentValue.EffectiveQuarantineDirectory
            + Path.DirectorySeparatorChar;
        var full = Path.GetFullPath(quarantinePath);
        var at = full.LastIndexOf(marker, NameComparison);
        return at <= 0 ? null : full[..at];
    }

    /// <summary>
    /// The only code in this feature that really deletes, and it goes on age rather than on
    /// which action produced the file. Retention 0 means never sweep.
    /// </summary>
    public int Sweep(string musicRoot)
    {
        var days = _settings.CurrentValue.EffectiveQuarantineRetentionDays;
        if (days <= 0) return 0;

        var root = RootFor(musicRoot);
        if (!Directory.Exists(root)) return 0;
        // A trash folder, or a dated folder in it, that is a link is not swept: it may lead out of the library.
        if (LinkBelow(musicRoot, Path.Combine(root, "x")) is not null) return 0;

        var cutoff = DateTime.UtcNow.AddDays(-days);
        var removed = 0;
        try
        {
            foreach (var dated in Directory.EnumerateDirectories(root))
            {
                var name = Path.GetFileName(dated);
                if (!DateTime.TryParseExact(name, "yyyy-MM-dd", null,
                        System.Globalization.DateTimeStyles.AssumeUniversal
                        | System.Globalization.DateTimeStyles.AdjustToUniversal, out var day)) continue;
                if (day >= cutoff) continue;
                if (LinkBelow(musicRoot, Path.Combine(dated, "x")) is not null) continue;

                var count = Directory.EnumerateFiles(dated, "*", SearchOption.AllDirectories)
                    .Count(path => !path.EndsWith(ManifestSuffix, StringComparison.Ordinal));
                Directory.Delete(dated, recursive: true);
                removed += count;
                _logger.LogInformation("Library action quarantine swept {Day} ({Count} file(s))", name, count);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning("Library action quarantine sweep failed: {M}", ex.Message);
        }
        return removed;
    }

    /// <summary>One sidecar into the trash beside its song. False, and left where it is, when it
    /// is outside the music folder, a link, or anything is in the way.</summary>
    private bool MoveSidecar(string from, string to, string musicRoot)
    {
        try
        {
            if (Outside(from, musicRoot) is { } refused)
            {
                _logger.LogInformation("Library action left {Path} where it is: {Why}", from, refused);
                return false;
            }
            if (File.Exists(to)) return false;
            if (LinkBelow(musicRoot, to) is not null) return false;
            try
            {
                File.Move(from, to);
            }
            catch (IOException)
            {
                if (!CopyVerified(from, to, new FileInfo(from).Length)) return false;
                File.Delete(from);
            }
            return true;
        }
        catch (Exception ex)
        {
            _logger.LogWarning("Library action could not move {Path} with its song: {M}", from, ex.Message);
            return false;
        }
    }

    /// <summary>Writes into a trashed song's note which outside songs Octo knew it as, so Put back
    /// can tell the library again.</summary>
    public void RecordMappings(string quarantinePath, IReadOnlyList<Octo.Services.Local.LocalSongMapping> mappings)
    {
        if (mappings.Count == 0 || ReadManifest(quarantinePath) is not { } manifest) return;
        WriteManifest(quarantinePath, manifest with { Mappings = mappings });
    }

    /// <summary>The names of the song's own files (its lyrics) waiting in the trash with it, which
    /// a restore puts back too. Empty when there are none, or the manifest cannot be read.</summary>
    public IReadOnlyList<string> SidecarNames(string quarantinePath) =>
        (ReadManifest(quarantinePath)?.Sidecars ?? [])
            .Where(sidecar => File.Exists(sidecar.QuarantinePath))
            .Select(sidecar => Path.GetFileName(sidecar.OriginalPath))
            .ToList();

    private void WriteManifest(string quarantinePath, QuarantineManifest manifest)
    {
        try
        {
            File.WriteAllText(quarantinePath + ManifestSuffix, JsonSerializer.Serialize(manifest));
        }
        catch (Exception ex)
        {
            // The journal still records this, so a missing manifest costs the standalone
            // restore rather than the recovery entirely.
            _logger.LogWarning("Library action could not write a quarantine manifest: {M}", ex.Message);
        }
    }

    private QuarantineManifest? ReadManifest(string quarantinePath)
    {
        try
        {
            var path = quarantinePath + ManifestSuffix;
            if (!File.Exists(path)) return null;
            return JsonSerializer.Deserialize<QuarantineManifest>(File.ReadAllText(path));
        }
        catch { return null; }
    }

    private static string Unique(string path)
    {
        if (!File.Exists(path)) return path;
        var directory = Path.GetDirectoryName(path)!;
        var stem = Path.GetFileNameWithoutExtension(path);
        var extension = Path.GetExtension(path);
        for (var suffix = 2; suffix < 1000; suffix++)
        {
            var candidate = Path.Combine(directory, $"{stem} ({suffix}){extension}");
            if (!File.Exists(candidate)) return candidate;
        }
        return Path.Combine(directory, $"{stem} ({Guid.NewGuid():N}){extension}");
    }

    private static void TryDelete(string path)
    {
        try { if (File.Exists(path)) File.Delete(path); } catch { /* best effort */ }
    }
}
