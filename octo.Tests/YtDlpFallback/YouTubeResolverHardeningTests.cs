using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.Logging;
using Octo.Services.YouTube;

namespace Octo.Tests;

/// <summary>
/// What the resolver lets through to the shim: only real video ids, search text cleaned in one
/// place, and no raw search text in a log line. Each entry point (search, meta, stream,
/// download) is checked, since the heart, the import trickle, a retry and Better quality all
/// reach the shim through them.
/// </summary>
public class YouTubeResolverHardeningTests
{
    private const string GoodId = "dQw4w9WgXcQ";

    private sealed class Recorder : HttpMessageHandler
    {
        public List<Uri> Requests { get; } = [];
        public string Body { get; set; } = "{\"video_id\":\"dQw4w9WgXcQ\",\"title\":\"t\",\"duration\":200,\"path\":\"/x.mp3\"}";
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
        {
            Requests.Add(request.RequestUri!);
            return Task.FromResult(new HttpResponseMessage(System.Net.HttpStatusCode.OK)
            {
                Content = new StringContent(Body),
            });
        }
    }

    private sealed class Factory(HttpMessageHandler handler) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => new(handler, disposeHandler: false);
    }

    private sealed class LogSink : ILogger<YouTubeResolver>
    {
        public List<string> Lines { get; } = [];
        public IDisposable? BeginScope<TState>(TState state) where TState : notnull => null;
        public bool IsEnabled(LogLevel logLevel) => true;
        public void Log<TState>(LogLevel logLevel, EventId eventId, TState state, Exception? exception,
            Func<TState, Exception?, string> formatter) =>
            Lines.Add(formatter(state, exception) + " " + exception);
    }

    private static (YouTubeResolver Resolver, Recorder Http, LogSink Log) Build(Recorder? http = null)
    {
        http ??= new Recorder();
        var log = new LogSink();
        var config = new ConfigurationBuilder().AddInMemoryCollection(
            new Dictionary<string, string?> { ["YouTube:ShimUrl"] = "http://shim.test" }).Build();
        return (new YouTubeResolver(new Factory(http), config, log), http, log);
    }

    [Theory]
    [InlineData("dQw4w9WgXcQ", true)]
    [InlineData("-abcdefghij", true)]
    [InlineData("_abcdefghi-", true)]
    [InlineData("", false)]
    [InlineData(null, false)]
    [InlineData("dQw4w9WgXc", false)]
    [InlineData("dQw4w9WgXcQQ", false)]
    [InlineData("dQw4w9WgXc\n", false)]
    [InlineData("dQw4w9WgXc ", false)]
    [InlineData("dQw4w9WgXc/", false)]
    [InlineData("dQw4w9WgX%Q", false)]
    [InlineData("dQw4w9WgXcé", false)]
    [InlineData("dQw4w9WgXc٣", false)]
    public void OnlyAnElevenCharacterAsciiIdIsAVideoId(string? id, bool expected) =>
        Assert.Equal(expected, YouTubeResolver.IsVideoId(id));

    [Fact]
    public void CleaningRemovesControlCharactersAndCapsTheLength()
    {
        Assert.Equal("a b c", YouTubeResolver.CleanQuery("  a\nb\r\n\tc \0"));
        Assert.Null(YouTubeResolver.CleanQuery("\n\t \0"));
        Assert.Null(YouTubeResolver.CleanQuery(null));
        Assert.Equal(YouTubeResolver.MaxQueryLength, YouTubeResolver.CleanQuery(new string('x', 5000))!.Length);
    }

    [Fact]
    public async Task SearchSendsCleanedTextOnly()
    {
        var (resolver, http, _) = Build();
        await resolver.SearchAsync("Artist\r\n--exec x\nTitle");
        var query = Uri.UnescapeDataString(http.Requests.Single().Query);
        Assert.DoesNotContain('\n', query);
        Assert.DoesNotContain('\r', query);
        Assert.Contains("q=Artist --exec x Title", query);
    }

    [Fact]
    public async Task SearchAndMetaSendNothingForTextThatCleansToNothing()
    {
        var (resolver, http, _) = Build();
        Assert.Null(await resolver.SearchAsync("\n\t"));
        Assert.Null(await resolver.MetaAsync("\0"));
        Assert.Empty(http.Requests);
    }

    [Fact]
    public async Task MetaCleansTheTextToo()
    {
        var (resolver, http, _) = Build();
        await resolver.MetaAsync(new string('y', 900) + "\n");
        Assert.True(Uri.UnescapeDataString(http.Requests.Single().Query).Length < 260);
    }

    [Fact]
    public async Task AHitWhoseIdIsNotAVideoIdIsDropped()
    {
        var http = new Recorder { Body = "{\"video_id\":\"../../etc/passwd\",\"title\":\"t\"}" };
        var (resolver, _, _) = Build(http);
        Assert.Null(await resolver.SearchAsync("a b"));
        Assert.Null(await resolver.MetaAsync("a b"));
    }

    [Theory]
    [InlineData("short")]
    [InlineData("dQw4w9WgXcQ&dest=/etc")]
    [InlineData("dQw4w9WgXc\n")]
    public async Task StreamAndDownloadRefuseABadIdWithoutCallingTheShim(string id)
    {
        var (resolver, http, _) = Build();
        Assert.Null(await resolver.OpenStreamAsync(id));
        Assert.Null(await resolver.DownloadAsync(id, "/music/.octo-incoming/x"));
        Assert.Empty(http.Requests);
    }

    [Fact]
    public async Task DownloadCleansArtistAndTitle()
    {
        var (resolver, http, _) = Build();
        await resolver.DownloadAsync(GoodId, "/music/.octo-incoming/x", "Art\nist", new string('t', 600));
        var query = Uri.UnescapeDataString(http.Requests.Single().Query);
        Assert.Contains("artist=Art ist", query);
        Assert.DoesNotContain('\n', query);
        Assert.True(query.Length < 400);
    }

    [Fact]
    public async Task NoLogLineCarriesTheSearchText()
    {
        var failing = new FailingFactory();
        var log = new LogSink();
        var config = new ConfigurationBuilder().AddInMemoryCollection(
            new Dictionary<string, string?> { ["YouTube:ShimUrl"] = "http://shim.test" }).Build();
        var resolver = new YouTubeResolver(failing, config, log);

        await resolver.SearchAsync("secret-artist secret-title");
        await resolver.DownloadAsync(GoodId, "/music/.octo-incoming/x", "secret-artist", "secret-title");

        Assert.NotEmpty(log.Lines);
        Assert.All(log.Lines, line => Assert.DoesNotContain("secret-", line));
    }

    private sealed class FailingFactory : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => new(new Throwing());
        private sealed class Throwing : HttpMessageHandler
        {
            protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct) =>
                throw new HttpRequestException("failed " + request.RequestUri);
        }
    }
}
