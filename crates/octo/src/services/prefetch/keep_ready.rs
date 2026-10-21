//! Playlists kept ready. For each listener, the playlists they pick, and the ones they play from
//! most, have their outside songs' previews copied ahead, so any player starts them at once, and
//! their first songs (and a few at random) fetched as FLAC like a mix kept ready.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use octo_core::common::Clock;
use octo_core::settings::SettingsStore;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::prefetch_service::PrefetchService;
use super::preview_copies::{Copying, PreviewCopies};
use crate::services::family::FamilyPlaylistStore;
use crate::services::i_music_metadata_service::IMusicMetadataService;
use crate::services::imports::{ImportService, import_streaming};
use crate::services::library::{ChartPlaylistService, GeneratedPlaylistService};
use crate::services::listening::ListeningLog;
use crate::services::local::ILocalLibraryService;
use crate::services::soulseek::ExternalIdRegistry;

/// Outside songs whose previews are copied, from the start of each list.
pub const PREVIEWS_FROM_START: usize = 50;
/// And at random from the rest of it.
pub const PREVIEWS_AT_RANDOM: usize = 50;
/// How many of a listener's most-played lists are kept ready.
pub const MOST_PLAYED_LISTS: usize = 3;
/// A list counts as played from with at least this many plays.
pub const MOST_PLAYED_FLOOR: usize = 3;
/// The plays that count for most played: those of this long.
pub const MOST_PLAYED_FOR: Duration = Duration::days(14);
/// At most this many new copies start in one pass.
const NEW_COPIES_PER_PASS: usize = 200;
/// How long a pass holds a list's copies: until the next pass, and some.
const HELD_FOR: Duration = Duration::hours(3);
/// How often the lists are looked at when nothing asks sooner.
const EVERY: StdDuration = StdDuration::from_secs(60 * 60);
/// How long a pass waits for a free fill before it asks again.
const SLOT_WAIT: StdDuration = StdDuration::from_secs(1);
/// A pass that has not finished its copies by then leaves the rest to the next one.
const PASS_LIMIT: StdDuration = StdDuration::from_secs(30 * 60);
/// Videos picked for songs that have none, at a time.
const VIDEOS_AT_ONCE: usize = 8;

/// What one listener keeps ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picks {
    /// The playlist ids they picked.
    #[serde(default)]
    pub lists: Vec<String>,
    /// Their most-played lists are kept ready too.
    #[serde(default = "yes")]
    pub most_played: bool,
}

fn yes() -> bool {
    true
}

impl Default for Picks {
    fn default() -> Self {
        Picks {
            lists: Vec::new(),
            most_played: true,
        }
    }
}

/// Every listener's picks, in `keep-ready.json`.
pub struct KeepReadyStore {
    path: Option<PathBuf>,
    users: Mutex<BTreeMap<String, Picks>>,
}

impl KeepReadyStore {
    /// `path`: the file, or none to keep the picks in memory only.
    pub fn new(path: Option<PathBuf>) -> Self {
        let users = path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        KeepReadyStore {
            path,
            users: Mutex::new(users),
        }
    }

    /// A listener's picks, the defaults when they never chose.
    pub fn picks(&self, user: &str) -> Picks {
        self.users.lock().get(&key(user)).cloned().unwrap_or_default()
    }

    /// Saves a listener's picks.
    pub fn set(&self, user: &str, picks: Picks) {
        let snapshot = {
            let mut users = self.users.lock();
            users.insert(key(user), picks);
            users.clone()
        };
        let Some(path) = &self.path else {
            return;
        };
        match serde_json::to_vec_pretty(&snapshot) {
            Ok(json) => {
                let temp = path.with_extension("json.tmp");
                if let Err(e) = std::fs::write(&temp, json).and_then(|()| std::fs::rename(&temp, path)) {
                    warn!("Could not save the playlists kept ready: {e}");
                }
            }
            Err(e) => warn!("Could not save the playlists kept ready: {e}"),
        }
    }

    /// The listeners who chose something, by key.
    pub fn users(&self) -> Vec<String> {
        self.users.lock().keys().cloned().collect()
    }
}

fn key(user: &str) -> String {
    user.trim().to_lowercase()
}

/// One playlist that can hold outside songs, with its songs' ids in list order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSongs {
    pub id: String,
    pub name: String,
    /// `import`, `mix`, `chart` or `family`.
    pub kind: &'static str,
    pub songs: Vec<String>,
}

/// The lists `plays` came from most: at most three, each with at least three plays.
pub fn most_played(lists: &[ListSongs], plays: &[String]) -> Vec<String> {
    let mut counts: Vec<(usize, usize)> = lists
        .iter()
        .enumerate()
        .map(|(i, list)| {
            let songs: HashSet<&str> = list.songs.iter().map(String::as_str).collect();
            (i, plays.iter().filter(|id| songs.contains(id.as_str())).count())
        })
        .filter(|(_, n)| *n >= MOST_PLAYED_FLOOR)
        .collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    counts
        .into_iter()
        .take(MOST_PLAYED_LISTS)
        .map(|(i, _)| lists[i].id.clone())
        .collect()
}

/// The first `from_start` of `songs`, then `at_random` of the rest, each once.
pub fn first_and_random(songs: &[String], from_start: usize, at_random: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let unique: Vec<&String> = songs.iter().filter(|id| seen.insert(id.as_str())).collect();
    let mut picked: Vec<String> = unique.iter().take(from_start).map(|id| (*id).clone()).collect();
    let mut rest: Vec<String> = unique.iter().skip(from_start).map(|id| (*id).clone()).collect();
    for i in (1..rest.len()).rev() {
        rest.swap(i, rand::random_range(0..=i));
    }
    picked.extend(rest.into_iter().take(at_random));
    picked
}

/// Where the lists, the plays and the songs' videos come from.
#[async_trait]
pub trait KeepReadySources: Send + Sync {
    /// The playlists of `user` that can hold outside songs.
    async fn lists(&self, user: &str) -> Vec<ListSongs>;
    /// The song of each play `user` started since `since`, or None when plays are not recorded.
    async fn plays_since(&self, user: &str, since: DateTime<Utc>) -> Option<Vec<String>>;
    /// Whether plays are recorded.
    fn recording(&self) -> bool;
    /// The listeners Octo has recorded plays of, as their names.
    fn listeners(&self) -> Vec<String>;
    /// Whether `id` is an outside song.
    fn is_outside(&self, id: &str) -> bool;
    /// The YouTube video of each song, picking one for a song that has none yet.
    async fn videos(&self, ids: &[String]) -> HashMap<String, String>;
    /// The video a song plays, when one is known; picks none.
    fn known_video(&self, id: &str) -> Option<String>;
}

/// The FLAC side of playlists kept ready: getting songs ahead.
#[async_trait]
pub trait KeepFlac: Send + Sync {
    async fn keep_ready(&self, songs: Vec<(String, Vec<String>)>);
    async fn flac_ready(&self, id: &str) -> bool;
    fn flac_bytes(&self) -> u64;
}

#[async_trait]
impl KeepFlac for PrefetchService {
    async fn keep_ready(&self, songs: Vec<(String, Vec<String>)>) {
        PrefetchService::keep_ready(self, songs).await;
    }

    async fn flac_ready(&self, id: &str) -> bool {
        PrefetchService::flac_ready(self, id).await
    }

    fn flac_bytes(&self) -> u64 {
        PrefetchService::flac_bytes(self)
    }
}

/// One list as the dashboard shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListView {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    /// Its outside songs.
    pub outside: usize,
    /// Of those, the ones with a preview copy.
    pub previews_ready: usize,
    /// And with a FLAC here.
    pub flac_ready: usize,
    pub picked: bool,
    /// Kept ready for being played from most.
    pub by_most_played: bool,
}

/// One listener's playlists kept ready, for the dashboard and the family page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepReadyView {
    pub user: String,
    /// Getting songs ahead is on; nothing is kept ready while it is off.
    pub enabled: bool,
    pub most_played: bool,
    /// Plays are recorded, so most played can be worked out.
    pub listening_on: bool,
    pub lists: Vec<ListView>,
    /// The bytes held for playlists kept ready, and the room they have.
    pub kept_bytes: u64,
    pub kept_cap: u64,
}

/// Why a choice was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownList(pub String);

/// What someone keeps ready, as the dashboard and the family page send it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepReadyChoice {
    /// Whose lists: the dashboard names a person; the family page always means the member.
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub lists: Vec<String>,
    #[serde(default = "yes")]
    pub most_played: bool,
}

impl Default for KeepReadyChoice {
    fn default() -> Self {
        KeepReadyChoice {
            user: None,
            lists: Vec::new(),
            most_played: true,
        }
    }
}

/// Keeps every listener's chosen playlists ready.
pub struct KeepReadyService {
    settings: Arc<SettingsStore>,
    store: Arc<KeepReadyStore>,
    sources: Arc<dyn KeepReadySources>,
    flac: Arc<dyn KeepFlac>,
    copies: Arc<PreviewCopies>,
    clock: Clock,
    wake: Notify,
    pass_lock: tokio::sync::Mutex<()>,
}

impl KeepReadyService {
    pub fn new(
        settings: Arc<SettingsStore>,
        store: Arc<KeepReadyStore>,
        sources: Arc<dyn KeepReadySources>,
        flac: Arc<dyn KeepFlac>,
        copies: Arc<PreviewCopies>,
        clock: Clock,
    ) -> Arc<Self> {
        Arc::new(KeepReadyService {
            settings,
            store,
            sources,
            flac,
            copies,
            clock,
            wake: Notify::new(),
            pass_lock: tokio::sync::Mutex::new(()),
        })
    }

    /// The room playlists kept ready have: the cache's size, less what the songs fetched ahead take.
    fn kept_cap(&self) -> u64 {
        let cap = self.settings.current().prefetch.effective_cache_bytes();
        cap.saturating_sub(self.flac.flac_bytes())
    }

    /// The lists `user` keeps ready, as (their lists, their picks, the ids kept for most played).
    async fn chosen(&self, user: &str) -> (Vec<ListSongs>, Picks, Vec<String>) {
        let lists = self.sources.lists(user).await;
        let picks = self.store.picks(user);
        let by_most_played = if picks.most_played {
            match self
                .sources
                .plays_since(user, self.clock.now() - MOST_PLAYED_FOR)
                .await
            {
                Some(plays) => most_played(&lists, &plays),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        (lists, picks, by_most_played)
    }

    /// One listener's lists, for the dashboard and the family page.
    pub async fn view(&self, user: &str) -> KeepReadyView {
        let (lists, picks, by_most_played) = self.chosen(user).await;
        let mut views = Vec::with_capacity(lists.len());
        for list in &lists {
            let outside: Vec<&String> = list
                .songs
                .iter()
                .filter(|id| self.sources.is_outside(id))
                .collect();
            let previews_ready = outside
                .iter()
                .filter(|id| {
                    self.sources
                        .known_video(id)
                        .is_some_and(|video| self.copies.is_ready(&video))
                })
                .count();
            let mut flac_ready = 0;
            for id in &outside {
                if self.flac.flac_ready(id).await {
                    flac_ready += 1;
                }
            }
            views.push(ListView {
                id: list.id.clone(),
                name: list.name.clone(),
                kind: list.kind,
                outside: outside.len(),
                previews_ready,
                flac_ready,
                picked: picks.lists.contains(&list.id),
                by_most_played: by_most_played.contains(&list.id),
            });
        }
        KeepReadyView {
            user: user.trim().to_string(),
            enabled: self.settings.current().prefetch.enabled,
            most_played: picks.most_played,
            listening_on: self.sources.recording(),
            lists: views,
            kept_bytes: self.copies.kept_bytes(),
            kept_cap: self.kept_cap(),
        }
    }

    /// Saves what `user` keeps ready. Every list must be one of theirs.
    pub async fn choose(&self, user: &str, lists: Vec<String>, most_played: bool) -> Result<(), UnknownList> {
        let theirs: HashSet<String> = self.sources.lists(user).await.into_iter().map(|l| l.id).collect();
        if let Some(unknown) = lists.iter().find(|id| !theirs.contains(*id)) {
            return Err(UnknownList(unknown.clone()));
        }
        let mut unique = Vec::new();
        for id in lists {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        self.store.set(
            user,
            Picks {
                lists: unique,
                most_played,
            },
        );
        self.wake.notify_one();
        Ok(())
    }

    /// The listeners a pass looks at: everyone who chose lists, and everyone Octo recorded plays of.
    fn listeners(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.store
            .users()
            .into_iter()
            .chain(self.sources.listeners())
            .filter(|user| !user.trim().is_empty() && seen.insert(key(user)))
            .collect()
    }

    /// One look at every listener's lists: their outside songs' previews copied (while nothing is
    /// playing too), and their first songs queued as FLAC, which start only when getting songs
    /// ahead may download.
    pub async fn pass(&self) {
        let _one = self.pass_lock.lock().await;
        let settings = self.settings.current().prefetch.clone();
        if !settings.enabled {
            return;
        }
        let now = self.clock.now();
        let count = settings.effective_keep_ready_count();
        let mut previews: Vec<String> = Vec::new();
        let mut flac: Vec<(String, Vec<String>)> = Vec::new();
        for user in self.listeners() {
            let (lists, picks, by_most_played) = self.chosen(&user).await;
            let mut theirs = Vec::new();
            for list in lists
                .iter()
                .filter(|l| picks.lists.contains(&l.id) || by_most_played.contains(&l.id))
            {
                let outside: Vec<String> = list
                    .songs
                    .iter()
                    .filter(|id| self.sources.is_outside(id))
                    .cloned()
                    .collect();
                previews.extend(first_and_random(
                    &outside,
                    PREVIEWS_FROM_START,
                    PREVIEWS_AT_RANDOM,
                ));
                theirs.extend(first_and_random(&outside, count, count));
            }
            if !theirs.is_empty() {
                flac.push((user.clone(), theirs));
            }
        }
        self.flac.keep_ready(flac).await;

        let mut seen = HashSet::new();
        previews.retain(|id| seen.insert(id.clone()));
        if previews.is_empty() {
            return;
        }
        let deadline = tokio::time::Instant::now() + PASS_LIMIT;
        let until = now + HELD_FOR;
        let cap = self.kept_cap();
        let mut started = 0;
        for batch in previews.chunks(VIDEOS_AT_ONCE) {
            let videos = self.sources.videos(batch).await;
            for id in batch {
                let Some(video) = videos.get(id) else {
                    continue;
                };
                loop {
                    if !self.copies.is_ready(video) && !self.copies.kept_room(cap) {
                        info!("Keep ready: the cache is full; {started} new copies this time");
                        return;
                    }
                    match self.copies.start_kept(video, until) {
                        Copying::Started => {
                            started += 1;
                            break;
                        }
                        Copying::Ready | Copying::Refused => break,
                        Copying::Busy if tokio::time::Instant::now() < deadline => {
                            tokio::time::sleep(SLOT_WAIT).await;
                        }
                        Copying::Busy => return,
                    }
                }
                if started >= NEW_COPIES_PER_PASS {
                    info!("Keep ready: {started} new copies this time, the rest next time");
                    return;
                }
            }
        }
        if started > 0 {
            info!(
                "Keep ready: {started} new copies of {} songs kept ready",
                previews.len()
            );
        }
    }

    /// The worker: a pass every hour, and soon after someone changes what they keep ready.
    pub async fn run(self: Arc<Self>, stopping: CancellationToken) -> anyhow::Result<()> {
        while !stopping.is_cancelled() {
            self.pass().await;
            tokio::select! {
                _ = stopping.cancelled() => break,
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(EVERY) => {}
            }
        }
        Ok(())
    }
}

/// The lists, plays and videos as the app has them.
pub struct AppKeepReadySources {
    pub settings: Arc<SettingsStore>,
    pub imports: Arc<ImportService>,
    pub registry: Arc<ExternalIdRegistry>,
    pub mixes: Arc<GeneratedPlaylistService>,
    pub charts: Arc<ChartPlaylistService>,
    pub family_playlists: Arc<FamilyPlaylistStore>,
    pub listening: Arc<ListeningLog>,
    pub metadata: Arc<dyn IMusicMetadataService>,
    pub library: Arc<dyn ILocalLibraryService>,
}

#[async_trait]
impl KeepReadySources for AppKeepReadySources {
    async fn lists(&self, user: &str) -> Vec<ListSongs> {
        let mut lists = Vec::new();
        for list in self.imports.streaming_lists(user) {
            let songs = list
                .tracks
                .iter()
                .map(|track| match &track.library_id {
                    Some(id) => id.clone(),
                    None => import_streaming::preview_song(&self.registry, track).id,
                })
                .collect();
            lists.push(ListSongs {
                id: import_streaming::playlist_id(&list),
                name: list.name.clone(),
                kind: "import",
                songs,
            });
        }
        for mix in self.mixes.current_lists(user) {
            // A mix drawn only when it is opened has no songs here until then.
            let songs = self
                .mixes
                .drawn(user, &mix)
                .unwrap_or_default()
                .iter()
                .filter_map(|song| song.get("id").and_then(|id| id.as_str()).map(str::to_string))
                .collect();
            lists.push(ListSongs {
                id: mix.id.clone(),
                name: mix.name.clone(),
                kind: "mix",
                songs,
            });
        }
        for chart in self.charts.lists_on() {
            let Some(list) = self.charts.made_playlist(user, &chart) else {
                continue;
            };
            let outside = self.charts.outside_songs(user, &chart);
            self.charts.keep_playable(&outside);
            lists.push(ListSongs {
                id: list.id.clone(),
                name: list.name.clone(),
                kind: "chart",
                songs: outside.into_iter().map(|song| song.id).collect(),
            });
        }
        if self.settings.current().family.enabled {
            let personal = format!("Personal:{}", user.trim());
            for playlist in self.family_playlists.list() {
                let songs: Vec<String> = playlist
                    .entries
                    .iter()
                    .filter(|e| {
                        !e.pending && (e.scope == "Shared" || e.scope.eq_ignore_ascii_case(&personal))
                    })
                    .filter_map(|e| e.library_id.clone().or_else(|| e.outside_id.clone()))
                    .collect();
                lists.push(ListSongs {
                    id: playlist.id.clone(),
                    name: playlist.name.clone(),
                    kind: "family",
                    songs,
                });
            }
        }
        lists
    }

    async fn plays_since(&self, user: &str, since: DateTime<Utc>) -> Option<Vec<String>> {
        if !self.listening.recording() {
            return None;
        }
        let listening = Arc::clone(&self.listening);
        let user = user.to_string();
        tokio::task::spawn_blocking(move || {
            listening
                .plays_of(&user)
                .into_iter()
                .filter(|play| play.played_at >= since)
                .map(|play| play.song_id)
                .collect()
        })
        .await
        .ok()
    }

    fn recording(&self) -> bool {
        self.listening.recording()
    }

    fn listeners(&self) -> Vec<String> {
        self.listening
            .people()
            .into_iter()
            .map(|(_, name)| name)
            .collect()
    }

    fn is_outside(&self, id: &str) -> bool {
        self.library.parse_song_id(id).0
    }

    async fn videos(&self, ids: &[String]) -> HashMap<String, String> {
        let missing: Vec<String> = ids
            .iter()
            .filter(|id| self.known_video(id).is_none())
            .cloned()
            .collect();
        if !missing.is_empty() {
            self.metadata.prewarm_next_streams(&missing, missing.len()).await;
        }
        ids.iter()
            .filter_map(|id| self.known_video(id).map(|video| (id.clone(), video)))
            .collect()
    }

    fn known_video(&self, id: &str) -> Option<String> {
        self.registry
            .lookup(id)
            .and_then(|routing| routing.snapshot().you_tube_id)
            .filter(|video| !video.is_empty())
    }
}
