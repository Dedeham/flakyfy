using Microsoft.AspNetCore.Mvc;
using Microsoft.Extensions.Options;
using Octo.Services.YtDlpFallback;

namespace Octo.Controllers;

/// <summary>The yt-dlp fallback for the dashboard: whether it is on and how many songs it is
/// counting or has given up on. Counts only, no songs and no people.</summary>
[ApiController]
[Route("api/admin/ytdlp-fallback")]
public sealed class YtDlpFallbackController(IOptionsMonitor<YtDlpFallbackSettings> settings, YtDlpFallbackStore store) : ControllerBase
{
    [HttpGet]
    public IActionResult Status()
    {
        var current = settings.CurrentValue.Normalized();
        if (!current.Enabled) return Ok(new { enabled = false });
        var given = store.GivenUp;
        var text = given.Count == 0
            ? null
            : $"{given.Count} song{(given.Count == 1 ? " was" : "s were")} given up on after {current.MaxWaitHours} hours without a source.";
        return Ok(new
        {
            enabled = true,
            waiting = store.Count,
            givenUp = given.Count,
            lastGivenUpUtc = given.LastUtc,
            failedAttempts = current.FailedAttempts,
            attemptSpacingHours = current.AttemptSpacingHours,
            maxWaitHours = current.MaxWaitHours,
            line = text,
        });
    }
}
