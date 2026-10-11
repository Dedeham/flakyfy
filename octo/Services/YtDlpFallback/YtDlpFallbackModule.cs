using Octo.Services.Common;

namespace Octo.Services.YtDlpFallback;

public static class YtDlpFallbackModule
{
    /// <summary>
    /// The yt-dlp fallback: a song no source can get is, after several counted tries, fetched
    /// from YouTube as an MP3. Always registered; the switch (YTDLP_FALLBACK_ENABLED) is read when
    /// it runs, and with it off nothing here makes a call, starts work or writes a file.
    /// </summary>
    public static IServiceCollection AddYtDlpFallbackModule(this IServiceCollection services, IConfiguration config)
    {
        services.Configure<YtDlpFallbackSettings>(config.GetSection("YtDlpFallback"));
        services.AddSingleton(sp => new YtDlpFallbackStore(
            // Beside settings.json, which Program.cs puts in /app/config. Not a setting: no path is configurable.
            Path.Combine("/app/config", "ytdlp-fallback.json"),
            sp.GetRequiredService<ILogger<YtDlpFallbackStore>>()));
        services.AddSingleton<YtDlpFallbackService>();
        services.AddSingleton<IAcquisitionFallback>(sp => sp.GetRequiredService<YtDlpFallbackService>());
        services.AddHostedService<YtDlpFallbackWorker>();
        return services;
    }
}
