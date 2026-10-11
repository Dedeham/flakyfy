using Octo.Services.Subsonic;

namespace Octo.Services.Common;

/// <summary>
/// Something that wants to know when a person's Subsonic request has been accepted by Navidrome,
/// so it can do a little work for that person right then: set the favorites an import is waiting
/// to set, build the week's list. Octo holds no sign-in of its own for users, so this is the only
/// moment their credential is available.
///
/// Rules for an implementation:
/// - Return quickly and never throw. Start slow work on its own task, with
///   <c>ExecutionContext.SuppressFlow()</c> so it carries nothing of this request.
/// - The credential is valid for this request only. Do not keep it: not in a field, a store, a
///   log line or a queue. Use it on the task you start and let it go. Call
///   <c>credential.WithoutPassword()</c> first if the work may run for long.
/// - It runs for every verified request, so decide cheaply whether there is anything to do.
///
/// Called by <see cref="VerifiedRequests.Notify"/>, from the places that already verify a sign-in
/// with Navidrome (getPlaylists and the octoImports calls). Register an implementation with
/// <c>services.AddSingleton&lt;IVerifiedRequestObserver, YourObserver&gt;()</c>.
/// </summary>
public interface IVerifiedRequestObserver
{
    /// <param name="user">The Navidrome user name the request signed in as.</param>
    /// <param name="credential">That person's sign-in, for calls made as them.</param>
    void OnVerifiedRequest(string user, SubsonicCredential credential);
}

/// <summary>Tells every registered <see cref="IVerifiedRequestObserver"/> about a verified request.</summary>
public static class VerifiedRequests
{
    /// <summary>
    /// Fire and forget: one observer failing never reaches the request or the others. A request
    /// with no user name (an API key alone) or no sign-in is skipped, since nobody can be named.
    /// </summary>
    public static void Notify(IServiceProvider services, IReadOnlyDictionary<string, string> parameters)
    {
        var user = parameters.GetValueOrDefault("u")?.Trim();
        if (string.IsNullOrEmpty(user) || SubsonicCredential.From(parameters) is not { } signedIn) return;
        // An API key or token beside a name is not a sign-in as that name; nobody is named.
        if (parameters.GetValueOrDefault("apiKey") is { Length: > 0 } || parameters.GetValueOrDefault("jwt") is { Length: > 0 }) return;
        // Observers never see the raw password: a token made from it, or nothing they could replay.
        var credential = signedIn.WithoutPassword();
        var logger = services.GetService<ILoggerFactory>()?.CreateLogger("Octo.VerifiedRequests");
        foreach (var observer in services.GetServices<IVerifiedRequestObserver>())
        {
            try { observer.OnVerifiedRequest(user, credential); }
            catch (Exception ex) { logger?.LogWarning("{Observer} failed on a verified request: {Reason}", observer.GetType().Name, ex.Message); }
        }
    }
}
