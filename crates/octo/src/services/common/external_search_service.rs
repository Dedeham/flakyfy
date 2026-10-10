//! Port of `Services/Common/ExternalSearchService.cs`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use octo_core::common::dotnet::{self, ordinal_ignore_case_key};
use octo_core::common::{SongIdentity, SupersedableBuildCoordinator};
use octo_core::last_fm::last_fm_search_cleanup::LastFmSearchCleanup;
use octo_core::last_fm::last_fm_search_ranking::LastFmSearchRanking;
use octo_core::models::domain::{Album, Song};
use octo_core::settings::{SettingsStore, SubsonicSettings};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::services::i_music_metadata_service::IMusicMetadataService;
use crate::services::last_fm::LastFmService;

/// Rows built per query, regardless of how many the caller wants.
///
/// It has to be a constant rather than the caller's target, or single-flight is
/// unsound: a client asking for 8 rows could win the race and hand 8 rows to a caller
/// that asked for 150. 60 is the number because that is where enrichment stops
/// (BackgroundEnrichLimit), so rows past it would ship as bare placeholders carrying a
/// fallback duration, and because the Navidrome-native search path already caps here.
pub const BUILD_SIZE: i32 = 60;

// track.search returns at most 50 rows, so padding whenever the list was short of
// BuildSize made the second, sequential Last.fm call on every search.
const THIN_SEARCH_THRESHOLD: usize = 20;

/// Deadline for one build. Last.fm has no configured HTTP timeout of its own, so
/// without this a single hung call would pin the query for every joined caller.
const BUILD_TIMEOUT: Duration = Duration::from_secs(10);

/// Deadline for one album build. Deezer's own client timeout already bounds
/// the call inside it; this is the ceiling on the whole build.
const ALBUM_BUILD_TIMEOUT: Duration = Duration::from_secs(10);

/// Builds the external (discovery) half of a search, once per query.
///
/// Every caller for the same query joins one execution and receives the same list. That
/// is not an optimization, it is what keeps the search3 fix from re-creating issue #8.
/// Clients routinely fire several search calls for one typed query, and registry ids are
/// deterministic, so those calls resolve to the *same* SoulseekRouting objects and would
/// each run the enrichment pipeline over them concurrently: three writers to a shared
/// int? duration, and three times the Deezer fan-out against a budget that is already the
/// tightest thing in the system.
///
/// The returned list is FROZEN (an `Arc` nobody can write through). Callers slice it and
/// serialize it, concurrently, without copying. Both Subsonic serializers and the native one
/// only read, and the star/download paths rebuild a Song from the registry rather than from a
/// search result. Any future "top up the enrichment because this caller wanted more rows"
/// belongs inside the build, not after it.
pub struct ExternalSearchService {
    // Amperfy (and most Subsonic clients' type-ahead) fires one search3 call per
    // keystroke, uncancelled. Plain SingleFlight only collapses two callers asking for
    // the SAME query; it does nothing for the sequence "cage", "cage t", "cage the",
    // where each keystroke is a distinct key racing the same rate-limited lane as the
    // query the user actually meant. SupersedableBuildCoordinator adds prefix-based
    // cancellation on top of that collapsing.
    song_builds: SupersedableBuildCoordinator<Arc<Vec<Song>>>,
    album_builds: SupersedableBuildCoordinator<Arc<Vec<Album>>>,
    metadata: Arc<dyn IMusicMetadataService>,
    last_fm: Option<Arc<LastFmService>>,
    /// `IOptionsMonitor<SubsonicSettings>?`: how long a build waits for lengths, read at every
    /// build.
    settings: Option<Arc<SettingsStore>>,
    /// The last finished builds, by query.
    recent_songs: RecentResults<Song>,
    recent_albums: RecentResults<Album>,
}

impl ExternalSearchService {
    pub const BUILD_SIZE: i32 = BUILD_SIZE;

    pub fn new(
        metadata: Arc<dyn IMusicMetadataService>,
        last_fm: Option<Arc<LastFmService>>,
        settings: Option<Arc<SettingsStore>>,
    ) -> Self {
        ExternalSearchService {
            song_builds: SupersedableBuildCoordinator::new(),
            album_builds: SupersedableBuildCoordinator::new(),
            metadata,
            last_fm,
            settings,
            recent_songs: RecentResults::new(RECENT_RESULTS_TTL, RECENT_RESULTS_MAX),
            recent_albums: RecentResults::new(RECENT_RESULTS_TTL, RECENT_RESULTS_MAX),
        }
    }

    /// Up to [`BUILD_SIZE`] enriched external songs for this query. Callers take
    /// the prefix they need; the list is shared and must not be mutated.
    pub async fn get(&self, query: &str) -> Arc<Vec<Song>> {
        if dotnet::is_blank(query) {
            return Arc::new(Vec::new());
        }
        // Only the key, not the radio switch. Discovery in the search bar is a different
        // feature from radio, and gating it on EnableRadio made turning radio off empty
        // the search results too.
        let Some(last_fm) = self.last_fm.clone().filter(|l| l.has_api_key()) else {
            return Arc::new(Vec::new());
        };

        let key = recent_key(query);
        if let Some(cached) = self.recent_songs.get(&key, Instant::now()) {
            // A copy: the cached list is shared and frozen like any build's. Rows whose video
            // was found after the build answered carry its length now.
            let mut songs = cached.as_ref().clone();
            self.metadata.refresh_song_lengths(&mut songs);
            debug!(
                "external search '{query}' answered from the last {} rows",
                songs.len()
            );
            return Arc::new(songs);
        }

        let metadata = self.metadata.clone();
        let wait = self.settings.as_ref().map_or_else(
            || SubsonicSettings::default().effective_search_duration_wait(),
            |s| s.current().subsonic.effective_search_duration_wait(),
        );
        let query_text = query.to_string();
        let songs = self
            .song_builds
            .run(
                query.trim(),
                move |token: CancellationToken| async move {
                    tokio::select! {
                        built = build(query_text, last_fm, metadata, wait) => Ok(Arc::new(built)),
                        () = token.cancelled() => Err(anyhow::anyhow!("The operation was canceled.")),
                    }
                },
                // The wait for lengths is on top of the deadline, so a long one never times
                // the whole build out.
                BUILD_TIMEOUT + wait,
                Arc::new(Vec::new()),
                // Discovery is an addition to search, never a precondition for it. The
                // steps inside the build are individually best-effort, but the deadline is
                // not, and a timeout must not take local results down with it.
                |q: &str, e: &anyhow::Error| debug!("external search '{q}' failed: {e}"),
            )
            .await;
        // A failed, timed-out or superseded build answers an empty list, which is not kept.
        if !songs.is_empty() {
            self.recent_songs.put(key, songs.clone(), Instant::now());
        }
        songs
    }

    /// Up to `limit` external albums for this query, via the same prefix-supersession as
    /// [`ExternalSearchService::get`]. Needs no Last.fm key: Deezer's album catalog is keyless.
    pub async fn get_albums(&self, query: &str, limit: i32) -> Arc<Vec<Album>> {
        if dotnet::is_blank(query) || limit <= 0 {
            return Arc::new(Vec::new());
        }
        let key = format!("{}|{limit}", recent_key(query));
        if let Some(cached) = self.recent_albums.get(&key, Instant::now()) {
            return cached;
        }
        let metadata = self.metadata.clone();
        let query_text = query.to_string();
        let albums = self
            .album_builds
            .run(
                query,
                move |token: CancellationToken| async move {
                    tokio::select! {
                        albums = metadata.search_album_rows(&query_text, limit) => Ok(Arc::new(albums)),
                        () = token.cancelled() => Err(anyhow::anyhow!("The operation was canceled.")),
                    }
                },
                ALBUM_BUILD_TIMEOUT,
                Arc::new(Vec::new()),
                |q: &str, e: &anyhow::Error| debug!("external album search '{q}' failed: {e}"),
            )
            .await;
        if !albums.is_empty() {
            self.recent_albums.put(key, albums.clone(), Instant::now());
        }
        albums
    }
}

/// How long a finished build answers the same query again.
pub const RECENT_RESULTS_TTL: Duration = Duration::from_secs(10 * 60);

/// How many queries' builds are kept; past it the oldest go first.
pub const RECENT_RESULTS_MAX: usize = 256;

/// The key a query's build is kept under: trimmed and lowercase, so "Adele " and "adele" share
/// one.
pub fn recent_key(query: &str) -> String {
    query.trim().to_lowercase()
}

/// The builds of the last queries, each kept for [`RECENT_RESULTS_TTL`], at most
/// [`RECENT_RESULTS_MAX`] of them. Typing the same words again, paging a result, or a client
/// asking twice then answers at once instead of asking Last.fm, Deezer and YouTube again.
pub struct RecentResults<T> {
    entries: Mutex<RecentEntries<T>>,
    ttl: Duration,
    max: usize,
}

struct RecentEntries<T> {
    by_key: HashMap<String, (Instant, Arc<Vec<T>>)>,
    /// Keys oldest first, each once.
    order: VecDeque<String>,
}

impl<T> RecentResults<T> {
    pub fn new(ttl: Duration, max: usize) -> Self {
        RecentResults {
            entries: Mutex::new(RecentEntries {
                by_key: HashMap::new(),
                order: VecDeque::new(),
            }),
            ttl,
            max,
        }
    }

    /// The build kept for `key`, unless it is older than the time to live at `now`.
    pub fn get(&self, key: &str, now: Instant) -> Option<Arc<Vec<T>>> {
        let mut entries = self.entries.lock();
        let (kept_at, value) = entries.by_key.get(key)?;
        if now.saturating_duration_since(*kept_at) < self.ttl {
            return Some(value.clone());
        }
        entries.by_key.remove(key);
        entries.order.retain(|k| k != key);
        None
    }

    /// Keeps `value` for `key` from `now`, replacing an older build of it; past the limit the
    /// oldest query goes.
    pub fn put(&self, key: String, value: Arc<Vec<T>>, now: Instant) {
        let mut entries = self.entries.lock();
        if entries.by_key.insert(key.clone(), (now, value)).is_some() {
            entries.order.retain(|k| *k != key);
        }
        entries.order.push_back(key);
        while entries.order.len() > self.max {
            if let Some(oldest) = entries.order.pop_front() {
                entries.by_key.remove(&oldest);
            }
        }
    }
}

/// Fans out to Last.fm, then fills in the metadata a client needs to render and play
/// the rows. Order:
///   1. track.search hits for the query as typed, most listened first
///   2. canonical artist's top tracks (in case (1) was thin: common for
///      single-word artist queries)
///
/// Deduped by `SongIdentity::match_key`, so the same track cannot appear twice however its
/// artist and title are written.
async fn build(
    query: String,
    last_fm: Arc<LastFmService>,
    metadata: Arc<dyn IMusicMetadataService>,
    duration_wait: Duration,
) -> Vec<Song> {
    let build_size = BUILD_SIZE as usize;
    // HashSet<string>(OrdinalIgnoreCase) over the match keys.
    let mut seen = HashSet::new();
    let mut collected: Vec<(String, String)> = Vec::new();

    // Last.fm's search carries rows from mislabelled scrobbles; put their names right first,
    // or they reach the results, the player and each listener's Last.fm as they are. Then
    // most listened first: track.search orders by fuzzy relevance, which puts a side project's
    // song ahead of the hit everybody means. Repeated versions of a song are one row, and rows
    // in another script than the query go last (see LastFmSearchRanking).
    let tracks = LastFmSearchRanking::rank(
        &query,
        LastFmSearchCleanup::clean(&last_fm.search_tracks(&query, 50.min(BUILD_SIZE * 2)).await),
    );
    for t in &tracks {
        let key = SongIdentity::match_key(&t.artist, &t.title);
        if seen.insert(ordinal_ignore_case_key(&key)) {
            collected.push((t.artist.clone(), t.title.clone()));
        }
        if collected.len() >= build_size {
            break;
        }
    }

    if collected.len() < THIN_SEARCH_THRESHOLD {
        // Use the first track-search hit's artist as the canonical anchor
        // for top-tracks padding. Falls back to the raw query string when
        // track.search came back empty.
        let anchor = tracks.first().map_or(query.as_str(), |t| t.artist.as_str());
        let top_tracks = last_fm.get_artist_top_tracks(anchor, BUILD_SIZE * 2).await;
        for t in &top_tracks {
            let key = SongIdentity::match_key(&t.artist, &t.title);
            if seen.insert(ordinal_ignore_case_key(&key)) {
                collected.push((t.artist.clone(), t.title.clone()));
            }
            if collected.len() >= build_size {
                break;
            }
        }
    }

    let mut songs = Vec::with_capacity(collected.len());
    for (artist, title) in &collected {
        if let Some(hit) = metadata
            .search_songs_by_artist_title(artist, title, 1, None)
            .await
            .into_iter()
            .next()
        {
            songs.push(hit);
        }
    }
    info!("External search '{query}' -> {} placeholder songs", songs.len());

    // Album/art/year from Deezer (fast), then the ACCURATE duration for the top of the
    // list from the real YouTube video (so the scrub bar matches the audio and the
    // client advances correctly), waited for at most `duration_wait`. Rows not resolved by
    // then go out with Deezer's length and are resolved in the background. Bounded + cached.
    metadata.enrich_external_songs(&mut songs).await;
    let durations = if duration_wait.is_zero() {
        None
    } else {
        Some(
            metadata
                .resolve_top_durations_within(&mut songs, duration_wait)
                .await,
        )
    };

    // Fire-and-forget: pre-resolve YouTube videoIds for the top hits so the first
    // /rest/stream click doesn't pay the cold yt-dlp double-call cost (ytsearch1: + -g,
    // 6-16s combined). Arpeggi cancels at ~10s and falls back to a local song; without
    // this, external playback is unreachable from that client. 12 is about what fits on
    // the first page of search results.
    //
    // It runs LAST on purpose. It used to run before enrichment, where it wrote a
    // videoId chosen with no duration hint while ResolveTopDurationsAsync was choosing
    // a different one using the Deezer duration: so for the top rows the two raced and
    // the loser could leave a song advertising the length of a video that would not be
    // the one played.
    //
    // The background work gets its own copy of the list: the songs this build returns stay
    // frozen, and in background mode the durations pass writes only the routing. The prewarm
    // waits for the rows the durations pass is still resolving.
    let background = songs.clone();
    let prewarm = metadata.clone();
    match durations {
        None => {
            tokio::spawn(resolve_durations_then_prewarm(prewarm, background));
        }
        Some(still_running) => {
            tokio::spawn(async move {
                if let Some(pass) = still_running {
                    let _ = pass.await;
                }
                prewarm.prewarm_you_tube_ids(&background, 12).await;
            });
        }
    }

    // Same reasoning, for cover art: a client renders the first screen of results a
    // moment after this returns, and without a prewarm each row's getCoverArt call
    // pays for the Deezer/iTunes/Last.fm chain itself, one row at a time. 24 covers
    // more than a screenful so scrolling a little still finds a warm cache.
    let covers = songs.clone();
    tokio::spawn(async move { metadata.prewarm_cover_art(&covers, 24).await });

    songs
}

/// The durations pass search did not wait for, then the videoId prewarm, in that order
/// for the reason given in `build`. It runs in background mode, so the songs this
/// build returned stay frozen.
async fn resolve_durations_then_prewarm(metadata: Arc<dyn IMusicMetadataService>, mut songs: Vec<Song>) {
    metadata.resolve_top_durations(&mut songs, true).await;
    metadata.prewarm_you_tube_ids(&songs, 12).await;
}

#[cfg(test)]
#[path = "external_search_service_tests.rs"]
mod tests;
