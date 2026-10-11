using Microsoft.Extensions.Configuration;
using Octo.Services.YtDlpFallback;

namespace Octo.Tests.YtDlpFallback;

public sealed class YtDlpFallbackSettingsTests
{
    [Fact]
    public void TheDefaultsAreOnThreeTriesSixHoursApartAndSeventyTwoHours()
    {
        var s = new YtDlpFallbackSettings().Normalized();
        Assert.True(s.Enabled);
        Assert.Equal(3, s.FailedAttempts);
        Assert.Equal(6, s.AttemptSpacingHours);
        Assert.Equal(72, s.MaxWaitHours);
    }

    [Theory]
    [InlineData(0, 2)]
    [InlineData(1, 2)]
    [InlineData(-5, 2)]
    [InlineData(int.MinValue, 2)]
    [InlineData(5, 5)]
    [InlineData(11, 10)]
    [InlineData(int.MaxValue, 10)]
    public void FailedAttemptsStaysInItsRange_ZeroAndNegativeNeverMeanFewer(int given, int expected) =>
        Assert.Equal(expected, new YtDlpFallbackSettings { FailedAttempts = given }.Normalized().FailedAttempts);

    [Theory]
    [InlineData(0, 1)]
    [InlineData(-1, 1)]
    [InlineData(int.MinValue, 1)]
    [InlineData(12, 12)]
    [InlineData(73, 72)]
    [InlineData(int.MaxValue, 72)]
    public void SpacingStaysInItsRange(int given, int expected) =>
        Assert.Equal(expected, new YtDlpFallbackSettings { AttemptSpacingHours = given }.Normalized().AttemptSpacingHours);

    [Theory]
    [InlineData(0)]
    [InlineData(-1)]
    [InlineData(int.MinValue)]
    [InlineData(1)]
    [InlineData(24)]
    public void TheWaitIsNeverShorterThanTheTriesNeed(int given)
    {
        var s = new YtDlpFallbackSettings { MaxWaitHours = given }.Normalized();
        Assert.True(s.MaxWaitHours >= s.FailedAttempts * s.AttemptSpacingHours + 6);
    }

    [Fact]
    public void TheComputedMinimumWinsOverTheCeiling_InSixtyFourBitMath()
    {
        var s = new YtDlpFallbackSettings
        {
            FailedAttempts = int.MaxValue, AttemptSpacingHours = int.MaxValue, MaxWaitHours = int.MaxValue,
        }.Normalized();
        Assert.Equal(10, s.FailedAttempts);
        Assert.Equal(72, s.AttemptSpacingHours);
        Assert.Equal(10 * 72 + 6, s.MaxWaitHours); // above the 720 ceiling: the minimum wins
    }

    [Fact]
    public void AHugeWaitIsCappedAtTheCeiling() =>
        Assert.Equal(720, new YtDlpFallbackSettings { MaxWaitHours = int.MaxValue }.Normalized().MaxWaitHours);

    [Fact]
    public void BindsFromConfigurationAndAnEmptyBindKeepsTheDefaults()
    {
        var config = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["YtDlpFallback:Enabled"] = "false", ["YtDlpFallback:FailedAttempts"] = "4",
            }).Build();
        var bound = config.GetSection("YtDlpFallback").Get<YtDlpFallbackSettings>()!;
        Assert.False(bound.Enabled);
        Assert.Equal(4, bound.FailedAttempts);
        Assert.Equal(6, bound.AttemptSpacingHours);
    }
}
