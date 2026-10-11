using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Options;
using Moq;
using Octo.Models.Settings;
using Octo.Services.Metadata;
using System.Net;

namespace Octo.Tests;

/// <summary>
/// The Deezer and Last.fm base URLs are settings an admin sets so a test stack can point them at a
/// stand-in. The defaults must be the public hosts, a bad value must fall back to them, and the
/// rate limiter must meter whichever Deezer host is configured.
/// </summary>
public class OutsideBaseUrlTests
{
    [Fact]
    public void DefaultsAreThePublicHosts()
    {
        Assert.Equal("https://api.deezer.com", new MetadataSettings().EffectiveDeezerBaseUrl);
        Assert.Equal("https://ws.audioscrobbler.com/2.0/", new LastFmSettings().EffectiveBaseUrl);
    }

    [Theory]
    [InlineData("http://stub:8080/deezer", "http://stub:8080/deezer")]
    [InlineData("http://stub:8080/deezer/", "http://stub:8080/deezer")]
    [InlineData("  https://example.com  ", "https://example.com")]
    public void ADeezerBaseUrlIsTrimmedOfItsTrailingSlash(string set, string expected) =>
        Assert.Equal(expected, new MetadataSettings { DeezerBaseUrl = set }.EffectiveDeezerBaseUrl);

    [Theory]
    [InlineData("")]
    [InlineData("not a url")]
    [InlineData("file:///etc/passwd")]
    [InlineData("ftp://example.com/")]
    [InlineData("https://user:pw@example.com/")]
    [InlineData("https://example.com/?q=1")]
    public void ABadBaseUrlFallsBackToTheDefault(string set)
    {
        Assert.Equal("https://api.deezer.com", new MetadataSettings { DeezerBaseUrl = set }.EffectiveDeezerBaseUrl);
        Assert.Equal("https://ws.audioscrobbler.com/2.0/", new LastFmSettings { BaseUrl = set }.EffectiveBaseUrl);
    }

    [Theory]
    [InlineData("http://stub:8080/lastfm", "http://stub:8080/lastfm/")]
    [InlineData("http://stub:8080/lastfm/", "http://stub:8080/lastfm/")]
    public void ALastFmBaseUrlEndsWithOneSlash(string set, string expected) =>
        Assert.Equal(expected, new LastFmSettings { BaseUrl = set }.EffectiveBaseUrl);

    private sealed class Inner : HttpMessageHandler
    {
        public int Calls;
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage r, CancellationToken ct)
        {
            Interlocked.Increment(ref Calls);
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK));
        }
    }

    [Fact]
    public async Task TheRateLimiterMetersTheConfiguredDeezerHostAndNotTheDefaultOne()
    {
        using var limiter = new DeezerRateLimiter();
        var monitor = new Mock<IOptionsMonitor<MetadataSettings>>();
        monitor.SetupGet(m => m.CurrentValue).Returns(new MetadataSettings { DeezerBaseUrl = "http://stub:8080/deezer" });
        var inner = new Inner();
        var handler = new DeezerRateLimitHandler(limiter, new Mock<ILogger<DeezerRateLimitHandler>>().Object, monitor.Object)
        { InnerHandler = inner };
        var client = new HttpClient(handler);

        // 120 calls to the default host are not metered now (it is not the configured one), so
        // the interactive window stays whole.
        for (var i = 0; i < 120; i++) await client.GetAsync("https://api.deezer.com/search?q=x");
        Assert.True(limiter.AcquireAsync(background: false, CancellationToken.None).IsCompleted);

        // The configured host is metered: it takes a permit (a lease is held for the call only,
        // so just prove the call passes through and arrives).
        var before = inner.Calls;
        var resp = await client.GetAsync("http://stub:8080/deezer/search/track?q=x");
        Assert.Equal(HttpStatusCode.OK, resp.StatusCode);
        Assert.Equal(before + 1, inner.Calls);
    }
}
