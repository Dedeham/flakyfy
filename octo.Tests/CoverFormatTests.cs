using Octo.Services.CoverArt;
using SixLabors.ImageSharp;
using SixLabors.ImageSharp.Formats;
using SixLabors.ImageSharp.Formats.Bmp;
using SixLabors.ImageSharp.Formats.Gif;
using SixLabors.ImageSharp.Formats.Jpeg;
using SixLabors.ImageSharp.Formats.Png;
using SixLabors.ImageSharp.Formats.Tiff;
using SixLabors.ImageSharp.Formats.Webp;
using SixLabors.ImageSharp.PixelFormats;

namespace Octo.Tests;

/// <summary>A cover from outside is decoded as JPEG, PNG, WebP or GIF only; other image parsers are not in reach.</summary>
public class CoverFormatTests
{
    private static byte[] Encoded(IImageEncoder encoder, int side = 300)
    {
        using var image = new Image<Rgba32>(side, side, new Rgba32(200, 80, 40));
        using var output = new MemoryStream();
        image.Save(output, encoder);
        return output.ToArray();
    }

    /// <summary>A little-endian TIFF header with an image directory at offset 8 that never ends.</summary>
    private static readonly byte[] BareTiffHeader = [0x49, 0x49, 0x2A, 0x00, 0x08, 0x00, 0x00, 0x00, 0xFF, 0xFF];

    public static TheoryData<string> Wanted => new() { "jpeg", "png", "webp", "gif" };

    private static byte[] Wanted_(string name) => name switch
    {
        "jpeg" => Encoded(new JpegEncoder()),
        "png" => Encoded(new PngEncoder()),
        "webp" => Encoded(new WebpEncoder()),
        _ => Encoded(new GifEncoder()),
    };

    [Theory]
    [MemberData(nameof(Wanted))]
    public void JPEG_PNG_WebP_and_GIF_covers_still_work(string name)
    {
        var cover = Wanted_(name);
        Assert.Equal((300, 300), CoverImage.Measure(cover));
        Assert.True(CoverImage.IsUsable(cover, requireSquare: true));
        Assert.NotNull(CoverImage.CropToSquare(cover));
        Assert.NotNull(CoverImage.LooksHash(cover));
        Assert.NotNull(CoverImage.Detail(cover, 128));
        Assert.True(CoverImage.ToJpeg(cover).AsSpan(0, 2).SequenceEqual(new byte[] { 0xFF, 0xD8 }));
        Assert.NotNull(CoverArtService.SwatchesOf(cover));
    }

    public static TheoryData<string> Refused => new() { "tiff", "bmp", "header" };

    private static byte[] Refused_(string name) => name switch
    {
        "tiff" => Encoded(new TiffEncoder()),
        "bmp" => Encoded(new BmpEncoder()),
        _ => BareTiffHeader,
    };

    [Theory]
    [MemberData(nameof(Refused))]
    public void A_cover_in_another_format_is_treated_as_unreadable_on_every_path(string name)
    {
        var cover = Refused_(name);
        Assert.False(CoverImage.WithinLimits(cover));
        Assert.Null(CoverImage.Measure(cover));
        Assert.False(CoverImage.IsUsable(cover, requireSquare: false));
        Assert.Null(CoverImage.CropToSquare(cover));
        Assert.Null(CoverImage.LooksHash(cover));
        Assert.Null(CoverImage.Detail(cover, 128));
        Assert.Same(cover, CoverImage.ToJpeg(cover));
        Assert.Same(cover, CoverImage.FitWithin(cover, 100));
        Assert.Null(CoverArtService.SwatchesOf(cover));
        Assert.Equal("image/jpeg", CoverImage.MimeType(cover));
    }

    [Fact]
    public void A_file_in_another_format_is_treated_as_unreadable()
    {
        var path = Path.Combine(Path.GetTempPath(), "octo-format-" + Guid.NewGuid().ToString("N") + ".jpg");
        try
        {
            File.WriteAllBytes(path, Encoded(new TiffEncoder()));
            Assert.False(CoverImage.WithinLimits(path));
            File.WriteAllBytes(path, Encoded(new PngEncoder()));
            Assert.True(CoverImage.WithinLimits(path));
        }
        finally
        {
            File.Delete(path);
        }
    }

    [Fact]
    public void An_outside_cover_in_another_format_gets_no_badge()
    {
        var tiff = Encoded(new TiffEncoder());
        var service = new CoverArtService(Microsoft.Extensions.Logging.Abstractions.NullLogger<CoverArtService>.Instance);
        Assert.Same(tiff, service.AddOctoBadge(tiff));
    }
}
