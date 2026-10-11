using System.Net;
using Microsoft.Extensions.Options;
using Octo.Models.Settings;

namespace Octo.Services.Metadata;

/// <summary>
/// Spends a <see cref="DeezerRateLimiter"/> permit before each Deezer API call.
/// Transient by design: IHttpClientFactory recycles handler chains, so the limiter
/// itself is the singleton and this is just the seam that consults it.
/// </summary>
public sealed class DeezerRateLimitHandler : DelegatingHandler
{
    /// <summary>Marks a request as background work, so cache warming yields to anything
    /// a user is actually waiting on.</summary>
    public static readonly HttpRequestOptionsKey<bool> BackgroundLane = new("octo.deezer.background");


    private readonly DeezerRateLimiter _limiter;
    private readonly ILogger<DeezerRateLimitHandler> _logger;
    private readonly IOptionsMonitor<MetadataSettings>? _settings;

    public DeezerRateLimitHandler(DeezerRateLimiter limiter, ILogger<DeezerRateLimitHandler> logger,
        IOptionsMonitor<MetadataSettings>? settings = null)
    {
        _limiter = limiter;
        _logger = logger;
        _settings = settings;
    }

    /// <summary>The metered host: the configured Deezer API host (api.deezer.com by default).</summary>
    private string ApiHost => new Uri(
        (_settings?.CurrentValue ?? new MetadataSettings()).EffectiveDeezerBaseUrl).Host;

    protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken ct)
    {
        // Only the API is metered. The cover-art lookup pulls actual image bytes from
        // cdn-images.dzcdn.net through this same client, and that host has no quota:
        // metering it would spend an API permit per rendered row and throttle a CDN for
        // nothing.
        if (!string.Equals(request.RequestUri?.Host, ApiHost, StringComparison.OrdinalIgnoreCase))
            return await base.SendAsync(request, ct);

        var background = request.Options.TryGetValue(BackgroundLane, out var bg) && bg;

        using var lease = await _limiter.AcquireAsync(background, ct);
        if (!lease.IsAcquired)
        {
            // The queue is full. Answering 429 rather than throwing means callers take
            // their existing "Deezer refused this" path, which never caches the result —
            // so back-pressure can slow us down but can never poison the cache.
            _logger.LogWarning("deezer rate limiter rejected a {Lane} request to {Url}",
                background ? "background" : "interactive", request.RequestUri);
            return new HttpResponseMessage(HttpStatusCode.TooManyRequests) { RequestMessage = request };
        }

        return await base.SendAsync(request, ct);
    }
}
