namespace Octo.Models.Settings;

/// <summary>
/// Listen submission for tracks that never touch Navidrome's library. Plays of local
/// tracks are scrobbled by Navidrome itself (Octo relays every scrobble unchanged);
/// plays of external tracks, previews from search and Continuous Radio, would
/// otherwise be lost to the listener's history. Each listener authorises with their
/// own ListenBrainz user token; a single default token covers a one-person install.
/// Every value is read through IOptionsMonitor at submit time.
/// </summary>
public class ListenBrainzSettings
{
    /// <summary>
    /// Default user token, used for any Navidrome user without an entry in
    /// <see cref="UserTokens"/>. From listenbrainz.org/settings. Empty disables
    /// submission for users without their own token.
    /// Environment variable: LISTENBRAINZ__TOKEN
    /// </summary>
    public string Token { get; set; } = "";

    /// <summary>
    /// Navidrome username to ListenBrainz user token, for installs with more than one
    /// listener. Environment variable form: LISTENBRAINZ__USERTOKENS__alice=...
    /// </summary>
    public Dictionary<string, string> UserTokens { get; set; } = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>Submit a listen when an external track completes (a scrobble for an
    /// external id, or a Continuous Radio track played to the end).</summary>
    public bool SubmitExternalPlays { get; set; } = true;

    /// <summary>
    /// Where ListenBrainz's API is. Only a test stack changes this, so a stand-in can answer; any value that
    /// is not an http(s) address is ignored. Environment variable: LISTENBRAINZ__APIURL
    /// </summary>
    public string ApiUrl { get; set; } = DefaultApiUrl;

    /// <summary>Where ListenBrainz's labs datasets are; same rules as <see cref="ApiUrl"/>.
    /// Environment variable: LISTENBRAINZ__LABSURL</summary>
    public string LabsUrl { get; set; } = DefaultLabsUrl;

    public const string DefaultApiUrl = "https://api.listenbrainz.org";
    public const string DefaultLabsUrl = "https://labs.api.listenbrainz.org";

    /// <summary><see cref="ApiUrl"/> without a trailing slash, or the default when it is not an http(s) address.</summary>
    public string ApiBase => Base(ApiUrl, DefaultApiUrl);

    /// <summary><see cref="LabsUrl"/> without a trailing slash, or the default when it is not an http(s) address.</summary>
    public string LabsBase => Base(LabsUrl, DefaultLabsUrl);

    private static string Base(string? value, string fallback) =>
        Uri.TryCreate(value?.Trim(), UriKind.Absolute, out var uri) && uri.Scheme is "http" or "https"
            ? uri.GetLeftPart(UriPartial.Path).TrimEnd('/') : fallback;

    /// <summary>The token that applies to this listener, or null when none is configured.</summary>
    public string? TokenFor(string username)
    {
        if (!SubmitExternalPlays) return null;
        if (!string.IsNullOrWhiteSpace(username)
            && UserTokens.TryGetValue(username.Trim(), out var own)
            && !string.IsNullOrWhiteSpace(own))
            return own.Trim();
        return string.IsNullOrWhiteSpace(Token) ? null : Token.Trim();
    }
}
