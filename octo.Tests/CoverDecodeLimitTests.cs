using Octo.Services.CoverArt;
using SixLabors.ImageSharp;
using SixLabors.ImageSharp.Formats.Png;
using SixLabors.ImageSharp.PixelFormats;

namespace Octo.Tests;

/// <summary>A cover from outside is checked against a size cap before its pixels are decoded.</summary>
public class CoverDecodeLimitTests
{
    private static byte[] Png(int width, int height)
    {
        using var image = new Image<L8>(width, height);
        using var output = new MemoryStream();
        image.Save(output, new PngEncoder());
        return output.ToArray();
    }

    [Fact]
    public void An_ordinary_cover_is_within_the_limits()
    {
        var cover = CoverKeepTests.Art(1, 600);
        Assert.True(CoverImage.WithinLimits(cover));
        Assert.Equal((600, 600), CoverImage.Measure(cover));
        Assert.NotNull(CoverImage.LooksHash(cover));
        Assert.NotNull(CoverImage.CropToSquare(cover));
    }

    [Fact]
    public void A_picture_over_the_byte_limit_is_not_read()
    {
        var huge = new byte[CoverImage.MaxBytes + 1];
        Assert.False(CoverImage.WithinLimits(huge));
        Assert.Null(CoverImage.Measure(huge));
        Assert.False(CoverImage.IsUsable(huge, requireSquare: false));
        Assert.Null(CoverImage.LooksHash(huge));
    }

    [Fact]
    public void A_picture_with_a_side_over_the_limit_is_not_decoded()
    {
        var wide = Png(CoverImage.MaxSide + 1, 1);
        Assert.False(CoverImage.WithinLimits(wide));
        Assert.Null(CoverImage.Measure(wide));
        Assert.Null(CoverImage.CropToSquare(wide));
        Assert.Null(CoverImage.LooksHash(wide));
        Assert.Null(CoverImage.Detail(wide, 64));
        Assert.Same(wide, CoverImage.ToJpeg(wide));
        Assert.Same(wide, CoverImage.FitWithin(wide, 100));
    }

    [Fact]
    public void A_picture_with_too_many_pixels_is_not_decoded()
    {
        var side = (int)Math.Sqrt(CoverImage.MaxPixels) + 1;
        var big = Png(side, side);
        Assert.False(CoverImage.WithinLimits(big));
        Assert.Null(CoverImage.Measure(big));
        Assert.Null(CoverImage.CropToSquare(big));
    }

    [Fact]
    public void A_file_over_the_limits_is_refused_and_one_within_them_is_not()
    {
        var folder = Path.Combine(Path.GetTempPath(), "octo-limit-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(folder);
        try
        {
            var small = Path.Combine(folder, "small.png");
            var wide = Path.Combine(folder, "wide.png");
            File.WriteAllBytes(small, Png(400, 400));
            File.WriteAllBytes(wide, Png(CoverImage.MaxSide + 1, 1));
            Assert.True(CoverImage.WithinLimits(small));
            Assert.False(CoverImage.WithinLimits(wide));
            Assert.False(CoverImage.WithinLimits(Path.Combine(folder, "missing.png")));
        }
        finally
        {
            Directory.Delete(folder, true);
        }
    }
}
