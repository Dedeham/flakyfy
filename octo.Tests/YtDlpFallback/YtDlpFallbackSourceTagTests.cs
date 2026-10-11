using Octo.Services.Common;

namespace Octo.Tests.YtDlpFallback;

/// <summary>The file of a fallback copy says where it came from, in the file itself, and says nothing about who asked.</summary>
public sealed class YtDlpFallbackSourceTagTests : IDisposable
{
    private readonly string _dir = Path.Combine(Path.GetTempPath(), "fla21t-" + Guid.NewGuid().ToString("N"));

    public YtDlpFallbackSourceTagTests() => Directory.CreateDirectory(_dir);
    public void Dispose() { try { Directory.Delete(_dir, true); } catch { } }

    [Fact]
    public void TheMp3CarriesTheMarkAndTheVideoIdAndNothingElseAboutTheRequest()
    {
        var path = Path.Combine(_dir, "a.mp3");
        File.WriteAllBytes(path, AudioFixtures.Mp3());
        using (var file = TagLib.File.Create(path))
        {
            TagWriterExtras.SetSourceMark(file, AcquisitionMarks.YouTubeFallback, "dQw4w9WgXcQ");
            file.Save();
        }

        using var read = TagLib.File.Create(path);
        var id3 = (TagLib.Id3v2.Tag)read.GetTag(TagLib.TagTypes.Id3v2);
        Assert.Equal("youtube-fallback", TagLib.Id3v2.UserTextInformationFrame.Get(id3, "OCTO_SOURCE", false).Text.Single());
        Assert.Equal("dQw4w9WgXcQ", TagLib.Id3v2.UserTextInformationFrame.Get(id3, "OCTO_SOURCE_ID", false).Text.Single());
        var userFrames = id3.GetFrames<TagLib.Id3v2.UserTextInformationFrame>().Select(f => f.Description).OrderBy(d => d).ToArray();
        Assert.Equal(new[] { "OCTO_SOURCE", "OCTO_SOURCE_ID" }, userFrames);
    }

    [Fact]
    public void AnUnmarkedSongGetsNoFrames()
    {
        var path = Path.Combine(_dir, "b.mp3");
        File.WriteAllBytes(path, AudioFixtures.Mp3());
        using (var file = TagLib.File.Create(path))
        {
            TagWriterExtras.SetSourceMark(file, null, null);
            file.Save();
        }
        Assert.DoesNotContain("OCTO_SOURCE", File.ReadAllText(path, System.Text.Encoding.Latin1));
    }

    [Fact]
    public void ForTheFallbackOnlyTheMarkIsRegisteredWhileItsDownloadRuns()
    {
        Assert.Null(AcquisitionMarks.Get("x"));
        AcquisitionMarks.Mark("x", AcquisitionMarks.YouTubeFallback);
        Assert.Equal("youtube-fallback", AcquisitionMarks.Get("x"));
        AcquisitionMarks.Unmark("x");
        Assert.Null(AcquisitionMarks.Get("x"));
        Assert.Null(AcquisitionMarks.Get(null));
    }
}
