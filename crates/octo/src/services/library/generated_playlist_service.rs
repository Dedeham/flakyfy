//! The generated playlists and their state file, `generated-playlists.json`.
//!
//! Genre and decade mixes built from the listener's own library and served like radio stations
//! (#54): per listener, read-only, under ids that start "og" so they can never be mistaken for a
//! Navidrome playlist. Nothing is written to Navidrome.
//!
//! Which mixes exist is decided from counts refreshed every RefreshHours, with hysteresis so a
//! genre sitting at the threshold does not appear and vanish on alternate days. What is in a mix
//! is a seeded draw per listener and period, so it holds still for the period and every client
//! sees the same tracks, then changes. The rules are
//! `octo_core::library::generated_playlist_service`.
//!
//! The Made for you lists (New Releases, Rediscover, Deep Cuts) live here too, on by default and
//! switched apart from the mixes: built in the same refresh, from one walk of the listener's
//! library as they see it, and kept on disk, so opening one never waits on a build. The picking
//! is `octo_core::library::for_you_lists`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, TimeDelta, Utc};
use indexmap::IndexMap;
use octo_core::common::{SingleFlight, dotnet};
use octo_core::json::dom::Node;
use octo_core::library::for_you_lists;
use octo_core::library::generated_playlist_service::{
    BLEND_CANDIDATES, FIRST_DECADE, MAX_GENRE_PAGES, POOL_PAGE, StateDocument, UserMixes, apply_hysteresis,
    blend, describe, dotnet_ticks, for_you_on, is_stale, kinds_of, mixes_on, parse_genres, period_index,
    playlist_id, seed, select, str_of, user_key,
};
use octo_core::models::domain::Song;
use octo_core::models::radio::{LastFmRadioStation, LastFmRadioStationKind};
use octo_core::settings::{ExplicitFilter, GeneratedPlaylistSettings, SettingsStore};
use octo_subsonic::subsonic_response_builder::SubsonicResponseBuilder;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub use octo_core::library::generated_playlist_service::GeneratedPlaylist;

use super::for_you_lists::{CatalogSeed, Entry, NewReleasesBuilder, mint};
use crate::services::framework::{DotnetDictionary, MemoryCache};
use crate::services::last_fm::OperationCanceled;
use crate::services::state_file;
use crate::services::subsonic::SubsonicProxyService;

const FIRST_LIST_WAIT: Duration = Duration::from_millis(1500);
const REFRESH_DEADLINE: Duration = Duration::from_secs(120);
// New Releases asks the catalog for up to NewReleaseArtists artists on its background lane.
const FOR_YOU_REFRESH_DEADLINE: Duration = Duration::from_secs(6 * 60);
/// A library bigger than this is walked this far: 40,000 songs.
pub const MAX_WALK_PAGES: i32 = 80;
const FOR_YOU_PREFIX: &str = "for:";
// Where a stored outside song of New Releases keeps what it is made from; never sent out.
const SEED_FIELD: &str = "octoCatalog";
const RETRY_AFTER_FAILURE: TimeDelta = TimeDelta::minutes(5);

/// The listeners' mixes and when each was last counted, under `_lock`.
#[derive(Default)]
struct Mixes {
    users: IndexMap<String, UserMixes>,
    last_attempt: HashMap<String, DateTime<Utc>>,
}

pub struct GeneratedPlaylistService {
    state_path: Option<PathBuf>,
    /// A background `SubsonicProxyService`, with no request behind it.
    proxy: SubsonicProxyService,
    /// The generated playlist and genre settings, read at use.
    settings: Arc<SettingsStore>,
    refreshes: SingleFlight<String, bool>,
    /// The draws, and the Discovery blend's candidates, until their period ends.
    drawn: MemoryCache<Arc<Vec<Node>>>,
    state: Mutex<Mixes>,
    /// What New Releases needs, used when it builds the list: None leaves New Releases as it
    /// was.
    for_you: Option<ForYouServices>,
}

/// The services New Releases is made with: the catalog walk, and the outside song's JSON.
pub struct ForYouServices {
    pub builder: Arc<NewReleasesBuilder>,
    pub responses: Arc<SubsonicResponseBuilder>,
}

impl GeneratedPlaylistService {
    /// Loads the counts from `state_path` (None or blank keeps them in memory only).
    pub fn new(
        state_path: Option<PathBuf>,
        proxy: SubsonicProxyService,
        settings: Arc<SettingsStore>,
    ) -> Self {
        let state_path = state_path.filter(|path| !dotnet::is_blank(&path.to_string_lossy()));
        let service = GeneratedPlaylistService {
            state_path,
            proxy,
            settings,
            refreshes: SingleFlight::new(),
            drawn: MemoryCache::new(256),
            state: Mutex::new(Mixes::default()),
            for_you: None,
        };
        service.load();
        service
    }

    /// The service with what New Releases needs.
    pub fn with_for_you(mut self, for_you: ForYouServices) -> Self {
        self.for_you = Some(for_you);
        self
    }

    /// The server's explicit filter, which New Releases' outside songs obey as search and radio's
    /// do. It is part of what decides the lists, so changing it makes them anew.
    fn explicit_filter_now(&self) -> ExplicitFilter {
        self.settings.current().subsonic.explicit_filter
    }

    /// The listener's mixes, refreshing the counts behind them when they are stale. The first
    /// list ever waits a moment for them, so mixes show on a listener's first look rather than
    /// their second; later refreshes run behind the answer.
    pub async fn list(
        self: &Arc<Self>,
        username: &str,
        auth: &IndexMap<String, String>,
    ) -> Vec<GeneratedPlaylist> {
        let settings = self.settings.current().generated_playlists.clone();
        if dotnet::is_blank(username) || !(mixes_on(&settings) || settings.any_for_you()) {
            return Vec::new();
        }

        let user = user_key(username);
        let now = Utc::now();
        let explicit_filter = self.explicit_filter_now();
        let (known, due) = {
            let mut state = self.state.lock();
            let current = state.users.get(&user);
            let known = current.is_some();
            let due = current.is_none_or(|current| is_stale(current, &settings, explicit_filter, now))
                && state
                    .last_attempt
                    .get(&user)
                    .is_none_or(|attempted| now - *attempted >= RETRY_AFTER_FAILURE);
            if due {
                state.last_attempt.insert(user.clone(), now);
            }
            (known, due)
        };

        if due {
            let service = Arc::clone(self);
            let name = username.to_string();
            let copy = auth.clone();
            let refresh = self.refreshes.run(
                user,
                move |token| async move { service.refresh(&name, &copy, &token).await },
                if settings.new_releases {
                    FOR_YOU_REFRESH_DEADLINE
                } else {
                    REFRESH_DEADLINE
                },
            );
            let name = username.to_string();
            let logged = tokio::spawn(async move {
                if let Err(error) = refresh.await {
                    debug!("Mix refresh failed for {name}: {error:#}");
                }
            });
            if !known {
                let _ = tokio::time::timeout(FIRST_LIST_WAIT, logged).await;
            }
        }
        self.current(username, &settings, Utc::now())
    }

    /// A mix by id, for this listener, as of now. None for anyone else's id.
    pub fn find(&self, username: &str, id: &str) -> Option<GeneratedPlaylist> {
        let settings = self.settings.current().generated_playlists.clone();
        if id.is_empty() || !id.starts_with("og") {
            return None;
        }
        self.current(username, &settings, Utc::now())
            .into_iter()
            .find(|mix| mix.id == id)
    }

    fn current(
        &self,
        username: &str,
        settings: &GeneratedPlaylistSettings,
        now_utc: DateTime<Utc>,
    ) -> Vec<GeneratedPlaylist> {
        let user = user_key(username);
        let hours = settings.effective_refresh_hours();
        let period = period_index(now_utc, hours);
        let start = DateTime::UNIX_EPOCH + TimeDelta::hours(period * i64::from(hours));
        let end = start + TimeDelta::hours(i64::from(hours));

        let state = self.state.lock();
        let Some(mixes) = state.users.get(&user) else {
            return Vec::new();
        };
        // Made for you first, newest to oldest, and only a list with something in it.
        let for_you = for_you_lists::KINDS
            .iter()
            .filter(|kind| {
                for_you_on(settings, kind) && mixes.for_you.get(**kind).is_some_and(|songs| !songs.is_empty())
            })
            .map(|kind| {
                let key = format!("{FOR_YOU_PREFIX}{kind}");
                GeneratedPlaylist {
                    id: playlist_id(username, &key),
                    key,
                    kind: kind.to_string(),
                    label: for_you_lists::name(kind).to_string(),
                    name: for_you_lists::name(kind).to_string(),
                    owner: username.trim().to_string(),
                    pool_size: mixes.for_you.get(*kind).map_or(0, Vec::len) as i32,
                    period_start_utc: start,
                    period_end_utc: end,
                }
            });
        let mixes_shown = mixes_on(settings);
        let list: Vec<GeneratedPlaylist> = for_you.collect();
        list.into_iter()
            .chain(
                mixes
                    .active
                    .iter()
                    .filter(|_| mixes_shown)
                    .filter(|key| {
                        (settings.genres && key.starts_with("genre:"))
                            || (settings.decades && key.starts_with("decade:"))
                    })
                    .map(|key| {
                        let (kind, label) = describe(key);
                        GeneratedPlaylist {
                            id: playlist_id(username, key),
                            key: key.clone(),
                            name: settings.name(&label),
                            kind,
                            label,
                            owner: username.trim().to_string(),
                            pool_size: mixes.counts.get(key).copied().unwrap_or(0),
                            period_start_utc: start,
                            period_end_utc: end,
                        }
                    }),
            )
            .collect()
    }

    /// The album covers New Releases' own cover is colored from, up to four.
    pub fn for_you_covers(&self, username: &str) -> Vec<String> {
        self.state
            .lock()
            .users
            .get(&user_key(username))
            .map(|state| state.for_you_covers.iter().take(4).cloned().collect())
            .unwrap_or_default()
    }

    /// Count what the listener's library holds per genre and decade, and decide which mixes they
    /// have. Only a complete count replaces the last one: a Navidrome that stops answering half
    /// way must not make every mix vanish.
    async fn refresh(
        &self,
        username: &str,
        auth: &IndexMap<String, String>,
        cancellation_token: &CancellationToken,
    ) -> anyhow::Result<bool> {
        let settings = self.settings.current().generated_playlists.clone();
        let mut counts: IndexMap<String, i32> = IndexMap::new();

        if mixes_on(&settings) && settings.genres {
            let Some(genres) = self.count_genres(auth).await else {
                return Ok(false);
            };
            counts.extend(genres);
        }
        if mixes_on(&settings) && settings.decades {
            let last = Utc::now().year() / 10 * 10;
            let mut decade = FIRST_DECADE;
            while decade <= last {
                if cancellation_token.is_cancelled() {
                    return Err(OperationCanceled.into());
                }
                let extra = [
                    ("size", POOL_PAGE.to_string()),
                    ("fromYear", decade.to_string()),
                    ("toYear", (decade + 9).to_string()),
                ];
                let Some(songs) = self
                    .fetch_songs(auth, "rest/getRandomSongs", "randomSongs", &extra)
                    .await
                else {
                    return Ok(false);
                };
                counts.insert(format!("decade:{decade}"), songs.len() as i32);
                decade += 10;
            }
        }

        let explicit_filter = self.explicit_filter_now();
        let before = self.state.lock().users.get(&user_key(username)).cloned();
        let (for_you, covers) = if settings.any_for_you() {
            self.build_for_you(
                username,
                auth,
                &settings,
                explicit_filter,
                before.as_ref(),
                cancellation_token,
            )
            .await?
        } else {
            (IndexMap::new(), Vec::new())
        };

        {
            let mut state = self.state.lock();
            let user = user_key(username);
            let previous = state
                .users
                .get(&user)
                .map(|old| old.active.clone())
                .unwrap_or_default();
            let active = apply_hysteresis(
                &counts,
                &previous,
                settings.effective_create_at(),
                settings.effective_remove_below(),
                settings.effective_max_playlists(),
            );
            state.users.insert(
                user,
                UserMixes {
                    active,
                    counts: counts.clone(),
                    counts_utc: Utc::now(),
                    kinds: kinds_of(&settings, explicit_filter),
                    for_you,
                    for_you_covers: covers,
                },
            );
            self.save_locked(&state);
        }
        info!(
            "Mixes for {username}: {} from {} genres and decades",
            counts.values().filter(|count| **count > 0).count(),
            counts.len()
        );
        Ok(true)
    }

    /// Made for you, from one walk of the listener's library as they see it. A list that could
    /// not be made this time (Navidrome or the catalog not answering) keeps what it had, so a bad
    /// day never empties it.
    async fn build_for_you(
        &self,
        username: &str,
        auth: &IndexMap<String, String>,
        settings: &GeneratedPlaylistSettings,
        explicit_filter: ExplicitFilter,
        before: Option<&UserMixes>,
        cancellation_token: &CancellationToken,
    ) -> anyhow::Result<(IndexMap<String, Vec<String>>, Vec<String>)> {
        let mut lists: IndexMap<String, Vec<String>> = IndexMap::new();
        let mut covers = before
            .map(|before| before.for_you_covers.clone())
            .unwrap_or_default();
        let keep_old = |lists: &mut IndexMap<String, Vec<String>>, kind: &str| {
            if let Some(old) = before.and_then(|before| before.for_you.get(kind)) {
                lists.insert(kind.to_string(), old.clone());
            }
        };

        let Some(library) = self.walk_library(auth, cancellation_token).await? else {
            for kind in for_you_lists::KINDS
                .iter()
                .filter(|kind| for_you_on(settings, kind))
            {
                keep_old(&mut lists, kind);
            }
            return Ok((lists, covers));
        };

        let now = Utc::now();
        let period = period_index(now, settings.effective_refresh_hours());
        let rediscover = for_you_lists::rediscover(
            &library,
            now,
            settings.effective_rediscover_months(),
            seed(
                username,
                &format!("{FOR_YOU_PREFIX}{}", for_you_lists::REDISCOVER_KIND),
                period,
            ),
        );
        if settings.rediscover {
            lists.insert(
                for_you_lists::REDISCOVER_KIND.to_string(),
                rediscover.iter().map(|song| song.to_json_string(false)).collect(),
            );
        }
        if settings.deep_cuts {
            let artists: Vec<String> = for_you_lists::top_artists(&library, for_you_lists::DEEP_CUT_ARTISTS)
                .into_iter()
                .map(|artist| artist.key)
                .collect();
            let excluded: std::collections::HashSet<String> = rediscover
                .iter()
                .filter_map(|song| str_of(song, "id").map(str::to_string))
                .collect();
            let deep_cuts = for_you_lists::deep_cuts(
                &library,
                &artists,
                &excluded,
                seed(
                    username,
                    &format!("{FOR_YOU_PREFIX}{}", for_you_lists::DEEP_CUTS_KIND),
                    period,
                ),
            );
            lists.insert(
                for_you_lists::DEEP_CUTS_KIND.to_string(),
                deep_cuts.iter().map(|song| song.to_json_string(false)).collect(),
            );
        }
        // Rediscover and Deep Cuts are ready in moments; New Releases waits on the catalog, a
        // minute or more the first time. Shown at once, a new listener's first lists never wait
        // on it.
        if settings.new_releases {
            self.publish_early(username, &lists);
            match &self.for_you {
                None => keep_old(&mut lists, for_you_lists::NEW_RELEASES_KIND),
                Some(services) => {
                    let built = services
                        .builder
                        .build(
                            &for_you_lists::top_artists(
                                &library,
                                settings.effective_new_release_artists() as usize,
                            ),
                            &library,
                            now.date_naive(),
                            settings.effective_new_release_weeks(),
                            cancellation_token,
                            explicit_filter,
                        )
                        .await?;
                    if built.entries.is_empty() && !built.whole {
                        keep_old(&mut lists, for_you_lists::NEW_RELEASES_KIND);
                    } else {
                        lists.insert(
                            for_you_lists::NEW_RELEASES_KIND.to_string(),
                            built
                                .entries
                                .iter()
                                .map(|entry| stored_entry(entry, &services.responses))
                                .collect(),
                        );
                        covers = Vec::new();
                        for cover in built.entries.iter().filter_map(|entry| entry.cover_url.clone()) {
                            if !covers.contains(&cover) {
                                covers.push(cover);
                            }
                            if covers.len() == 4 {
                                break;
                            }
                        }
                    }
                }
            }
        }
        info!(
            "Made for you for {username}: {} from {} library songs",
            lists
                .iter()
                .map(|(kind, songs)| format!("{} {}", for_you_lists::name(kind), songs.len()))
                .collect::<Vec<_>>()
                .join(", "),
            library.len()
        );
        Ok((lists, covers))
    }

    /// Lists shown before the whole build is done, without touching the lists the build is still
    /// reading. A listener with none yet gets a state that is still due, so the build finishes it.
    fn publish_early(&self, username: &str, lists: &IndexMap<String, Vec<String>>) {
        if lists.is_empty() {
            return;
        }
        let mut state = self.state.lock();
        let user = user_key(username);
        match state.users.get_mut(&user) {
            Some(mixes) => {
                for (kind, songs) in lists {
                    mixes.for_you.insert(kind.clone(), songs.clone());
                }
            }
            None => {
                state.users.insert(
                    user,
                    UserMixes {
                        for_you: lists.clone(),
                        ..Default::default()
                    },
                );
            }
        }
    }

    /// Every song of the listener's library as they see it, 500 at a time, up to
    /// [`MAX_WALK_PAGES`] pages; None when Navidrome did not answer, even part way: lists made
    /// from part of a library would show songs the listener owns as outside ones and miss their
    /// plays, so the lists they have stay instead.
    async fn walk_library(
        &self,
        auth: &IndexMap<String, String>,
        cancellation_token: &CancellationToken,
    ) -> anyhow::Result<Option<Vec<Node>>> {
        let mut all: Vec<Node> = Vec::new();
        for page in 0..MAX_WALK_PAGES {
            if cancellation_token.is_cancelled() {
                return Err(OperationCanceled.into());
            }
            let extra = [
                // Navidrome's whole library: an empty phrase, as the duplicate scan walks it.
                ("query", "\"\"".to_string()),
                ("songCount", POOL_PAGE.to_string()),
                ("songOffset", (page * POOL_PAGE).to_string()),
                ("albumCount", "0".to_string()),
                ("artistCount", "0".to_string()),
            ];
            let Some(songs) = self
                .fetch_songs(auth, "rest/search3", "searchResult3", &extra)
                .await
            else {
                return Ok(None);
            };
            let full = songs.len() as i32 >= POOL_PAGE;
            all.extend(songs);
            if !full {
                return Ok(Some(all));
            }
        }
        info!(
            "Made for you read the first {} songs of a bigger library",
            all.len()
        );
        Ok(Some(all))
    }

    /// A Made for you list as stored. Each outside song is made again from its seed, with the
    /// same id, so it plays even after a busy day has pushed it out of the outside songs Octo
    /// remembers; the seed itself is never sent out.
    fn stored_for_you(&self, username: &str, kind: &str) -> Vec<Node> {
        let stored: Option<Vec<String>> = self
            .state
            .lock()
            .users
            .get(&user_key(username))
            .and_then(|state| state.for_you.get(kind).cloned());
        let Some(stored) = stored else {
            return Vec::new();
        };
        let mut parsed: Vec<Node> = stored.iter().filter_map(|song| Node::parse(song).ok()).collect();
        for song in &mut parsed {
            let Some(fields) = song.as_object_mut() else {
                continue;
            };
            let Some(node) = fields.shift_remove(SEED_FIELD) else {
                continue;
            };
            if !node.is_object() {
                continue;
            }
            let Ok(seed) = serde_json::from_value::<CatalogSeed>(node.to_value()) else {
                continue;
            };
            if let Some(services) = &self.for_you {
                mint(services.builder.registry(), &seed);
            }
        }
        parsed
    }

    /// Genres by song count. Spellings that differ only in case are one genre, under the
    /// spelling with the most songs; years and the genre blocklist are never a mix of their own.
    async fn count_genres(&self, auth: &IndexMap<String, String>) -> Option<IndexMap<String, i32>> {
        let result = self
            .proxy
            .relay_safe("rest/getGenres", parameters(auth, &[]))
            .await?;
        if result.body.is_empty() {
            return None;
        }
        let root = Node::parse(&String::from_utf8_lossy(&result.body)).ok()?;
        let response = root.get("subsonic-response")?;
        if response.get("status").and_then(Node::as_str) != Some("ok") {
            return None;
        }
        let blocked = self.settings.current().genre.effective_blocklist();
        let rows = match response.get("genres").and_then(|genres| genres.get("genre")) {
            Some(Node::Array(rows)) => Some(rows.as_slice()),
            _ => None,
        };
        Some(parse_genres(rows, &blocked))
    }

    /// The songs of one mix for this period: the same for every request in the period.
    pub async fn materialize(
        &self,
        username: &str,
        playlist: &GeneratedPlaylist,
        auth: &IndexMap<String, String>,
        cancellation_token: &CancellationToken,
    ) -> anyhow::Result<Vec<Node>> {
        if for_you_lists::is_for_you(&playlist.kind) {
            return Ok(self.stored_for_you(username, &playlist.kind));
        }
        let cache_key = draw_key(username, playlist);
        if let Some(cached) = self.drawn.get(&cache_key) {
            return Ok(cached.as_ref().clone());
        }

        let settings = self.settings.current().generated_playlists.clone();
        let mut pool: Vec<Node> = Vec::new();
        if playlist.kind == "decade" {
            let decade: i32 = playlist
                .key
                .get("decade:".len()..)
                .unwrap_or_default()
                .parse()
                .map_err(|_| {
                    anyhow::anyhow!("The input string '{}' was not in a correct format.", playlist.key)
                })?;
            let extra = [
                ("size", POOL_PAGE.to_string()),
                ("fromYear", decade.to_string()),
                ("toYear", (decade + 9).to_string()),
            ];
            pool.extend(
                self.fetch_songs(auth, "rest/getRandomSongs", "randomSongs", &extra)
                    .await
                    .unwrap_or_default(),
            );
        } else {
            for page in 0..MAX_GENRE_PAGES {
                if cancellation_token.is_cancelled() {
                    return Err(OperationCanceled.into());
                }
                let extra = [
                    ("genre", playlist.label.clone()),
                    ("count", POOL_PAGE.to_string()),
                    ("offset", (page * POOL_PAGE).to_string()),
                ];
                let Some(songs) = self
                    .fetch_songs(auth, "rest/getSongsByGenre", "songsByGenre", &extra)
                    .await
                else {
                    break;
                };
                let full = songs.len() as i32 >= POOL_PAGE;
                pool.extend(songs);
                if !full {
                    break;
                }
            }
        }

        let period = period_index(playlist.period_start_utc, settings.effective_refresh_hours());
        let drawn = select(
            &pool,
            settings.effective_track_count(),
            settings.effective_max_per_artist(),
            settings.effective_new_share(),
            settings.effective_new_days(),
            Utc::now(),
            seed(username, &playlist.key, period),
        );
        if !pool.is_empty() {
            self.drawn.set(
                cache_key,
                Arc::new(drawn.clone()),
                1,
                until(playlist.period_end_utc),
            );
        }
        Ok(drawn)
    }

    /// The songs of one mix for this period if they have been drawn already, else None. Never
    /// fetches.
    pub fn drawn(&self, username: &str, playlist: &GeneratedPlaylist) -> Option<Vec<Node>> {
        if for_you_lists::is_for_you(&playlist.kind) {
            return Some(self.stored_for_you(username, &playlist.kind));
        }
        self.drawn
            .get(&draw_key(username, playlist))
            .map(|cached| cached.as_ref().clone())
    }

    /// Keep MIX_NEW_SHARE percent of Discovery Mix for library songs new to the listener.
    /// Discovery is otherwise all outside the library, so what someone added and never played was
    /// the one thing it could not surface. The station's songs come back as they were when there
    /// is nothing to put in.
    pub async fn blend_into_discovery(
        &self,
        username: &str,
        station: &LastFmRadioStation,
        songs: Vec<Song>,
        auth: &IndexMap<String, String>,
    ) -> Vec<Song> {
        let settings = self.settings.current().generated_playlists.clone();
        let share = settings.effective_new_share();
        if share == 0 || station.kind != LastFmRadioStationKind::Discovery || songs.is_empty() {
            return songs;
        }
        let wanted = (songs.len() as f64 * f64::from(share) / 100.0).round() as i32;
        if wanted == 0 {
            return songs;
        }

        // Only the candidates are cached, and applied to the station as it is now: a track that
        // became a library song since must not be put back as it was.
        let cache_key = format!(
            "blend|{}|{}|{}",
            user_key(username),
            station.id,
            dotnet_ticks(station.changed_utc)
        );
        let candidates = match self.drawn.get(&cache_key) {
            Some(candidates) => candidates,
            None => {
                let extra = [("size", BLEND_CANDIDATES.to_string())];
                let Some(fetched) = self
                    .fetch_songs(auth, "rest/getRandomSongs", "randomSongs", &extra)
                    .await
                else {
                    return songs;
                };
                let fetched = Arc::new(fetched);
                let now = Utc::now();
                let expires = if station.valid_until_utc > now {
                    station.valid_until_utc
                } else {
                    now + TimeDelta::hours(1)
                };
                self.drawn.set(cache_key, Arc::clone(&fetched), 1, until(expires));
                fetched
            }
        };
        match blend(
            &songs,
            &candidates,
            wanted,
            settings.effective_new_days(),
            Utc::now(),
        ) {
            Some(blended) => blended,
            None => songs,
        }
    }

    /// The songs of one Subsonic call, or None when it did not answer: an empty list is a real
    /// answer and None is not, and only the first may replace what is known.
    async fn fetch_songs(
        &self,
        auth: &IndexMap<String, String>,
        endpoint: &str,
        container: &str,
        extra: &[(&str, String)],
    ) -> Option<Vec<Node>> {
        let extra: Vec<(&str, &str)> = extra.iter().map(|(key, value)| (*key, value.as_str())).collect();
        let Some(result) = self.proxy.relay_safe(endpoint, parameters(auth, &extra)).await else {
            debug!("Mix lookup {endpoint} failed: no answer");
            return None;
        };
        if result.body.is_empty() {
            return None;
        }
        let root = match Node::parse(&String::from_utf8_lossy(&result.body)) {
            Ok(root) => root,
            Err(error) => {
                debug!("Mix lookup {endpoint} failed: {error}");
                return None;
            }
        };
        let response = root.get("subsonic-response")?;
        if response.get("status").and_then(Node::as_str) != Some("ok") {
            return None;
        }
        Some(
            match response.get(container).and_then(|songs| songs.get("song")) {
                Some(Node::Array(songs)) => songs.iter().filter(|song| song.is_object()).cloned().collect(),
                _ => Vec::new(),
            },
        )
    }

    fn load(&self) {
        let Some(path) = self.state_path.as_ref().filter(|path| path.is_file()) else {
            return;
        };
        let read = state_file::read_all_text(path)
            .map_err(|error| error.to_string())
            .and_then(|text| serde_json::from_str::<StateDocument>(&text).map_err(|error| error.to_string()));
        match read {
            Ok(document) => {
                let Some(users) = document.users else { return };
                let mut state = self.state.lock();
                for (user, mixes) in users {
                    state.users.insert(user, mixes);
                }
            }
            Err(message) => {
                // Only counts: the next list rebuilds them, so a bad file is set aside, not fatal.
                warn!("Mix state could not be read ({message}); it will be rebuilt");
                let mut corrupt = path.as_os_str().to_owned();
                corrupt.push(format!(".corrupt-{}", dotnet_ticks(Utc::now())));
                // Best effort.
                let _ = std::fs::rename(path, PathBuf::from(corrupt));
            }
        }
    }

    fn save_locked(&self, state: &Mixes) {
        let Some(path) = &self.state_path else { return };
        let document = StateDocument {
            users: Some(state.users.clone()),
        };
        if let Err(error) = state_file::save_atomic(path, &octo_core::json::to_string(&document)) {
            warn!("Mix state could not be written: {error}");
        }
    }
}

/// One entry of New Releases as stored: a library song as Navidrome sent it, or an outside song
/// with what it is made from.
fn stored_entry(entry: &Entry, responses: &SubsonicResponseBuilder) -> String {
    if let Some(library) = &entry.library {
        return library.to_json_string(false);
    }
    let Some(outside) = &entry.outside else {
        return Node::object().to_json_string(false);
    };
    let mut song = Node::from_value(&serde_json::Value::Object(
        responses.convert_song_to_json(outside),
    ));
    if let (Some(seed), Some(fields)) = (&entry.seed, song.as_object_mut())
        && let Ok(value) = serde_json::to_value(seed)
    {
        fields.insert(SEED_FIELD.to_string(), Node::from_value(&value));
    }
    song.to_json_string(false)
}

fn draw_key(username: &str, playlist: &GeneratedPlaylist) -> String {
    format!(
        "{}|{}|{}",
        user_key(username),
        playlist.id,
        dotnet_ticks(playlist.period_start_utc)
    )
}

/// An absolute expiry as the time left until it.
fn until(expires: DateTime<Utc>) -> Duration {
    (expires - Utc::now()).to_std().unwrap_or(Duration::ZERO)
}

/// The caller's own parameters, whatever auth they carry (a token, a password or an API key), as
/// the sync catalog does, minus anything that named the playlist being read. In insertion order
/// with a removed key's slot reused by the next key added, which is the order the query is
/// written in.
fn parameters(auth: &IndexMap<String, String>, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut parameters: DotnetDictionary<String> = DotnetDictionary::new();
    for (key, value) in auth {
        parameters.set(key.clone(), value.clone());
    }
    parameters.remove("id");
    parameters.remove("playlistId");
    for (key, value) in extra {
        parameters.set((*key).to_string(), (*value).to_string());
    }
    parameters.set("f".to_string(), "json".to_string());
    parameters
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[cfg(test)]
#[path = "generated_playlist_service_tests.rs"]
mod tests;
