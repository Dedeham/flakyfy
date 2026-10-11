namespace Octo.Services.YtDlpFallback;

/// <summary>
/// The yt-dlp fallback: a song that Soulseek and Lidarr both failed to get, several times over, is
/// fetched from YouTube as an MP3. Section <c>YtDlpFallback</c>; from the environment or
/// appsettings only, and always read through <see cref="Normalized"/>, so a bad value can never
/// mean "no limit".
/// </summary>
public sealed class YtDlpFallbackSettings
{
    public const int MinFailedAttempts = 2, MaxFailedAttempts = 10, DefaultFailedAttempts = 3;
    public const int MinSpacingHours = 1, MaxSpacingHours = 72, DefaultSpacingHours = 6;
    public const int MaxWaitCeilingHours = 720, DefaultMaxWaitHours = 72;

    /// <summary>YTDLP_FALLBACK_ENABLED. On by default. With the YouTube step in a person's own
    /// source list the fallback does nothing, so only chains without YouTube see it.</summary>
    public bool Enabled { get; set; } = true;

    /// <summary>YTDLP_FALLBACK_FAILED_ATTEMPTS. Counted chain runs that end empty before yt-dlp is
    /// used. 3 by default, at least 2.</summary>
    public int FailedAttempts { get; set; } = DefaultFailedAttempts;

    /// <summary>YTDLP_FALLBACK_ATTEMPT_SPACING_HOURS. The least time between two counted runs of
    /// the same song. 6 by default, at least 1.</summary>
    public int AttemptSpacingHours { get; set; } = DefaultSpacingHours;

    /// <summary>YTDLP_FALLBACK_MAX_WAIT_HOURS. How long a waiting heart is kept before it is given
    /// up. 72 by default, never below FailedAttempts x AttemptSpacingHours + 6.</summary>
    public int MaxWaitHours { get; set; } = DefaultMaxWaitHours;

    /// <summary>The settings inside their ranges. Each value goes to its own range first, then
    /// MaxWaitHours is raised to FailedAttempts x AttemptSpacingHours + 6, in 64-bit arithmetic.</summary>
    public YtDlpFallbackSettings Normalized()
    {
        var failed = Math.Clamp(FailedAttempts, MinFailedAttempts, MaxFailedAttempts);
        var spacing = Math.Clamp(AttemptSpacingHours, MinSpacingHours, MaxSpacingHours);
        var floor = (long)failed * spacing + 6;
        var wait = Math.Max(floor, Math.Clamp((long)MaxWaitHours, 1L, MaxWaitCeilingHours));
        return new YtDlpFallbackSettings
        {
            Enabled = Enabled,
            FailedAttempts = failed,
            AttemptSpacingHours = spacing,
            MaxWaitHours = (int)wait,
        };
    }

    // Not settings: limits in code, so they cannot be configured away.
    public const int PerHour = 12;
    public const int PerUserPerHour = 4;
    public const int MaxEntriesPerUser = 200;
    public const int MaxEntries = 5000;
    public const long MinFreeBytes = 5L * 1024 * 1024 * 1024;
    public const int DurationToleranceSeconds = 8;
}
