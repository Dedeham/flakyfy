//! Builds canonical station snapshots from bounded listening signals, asking Last.fm for the
//! candidates. The weighing, the draw and the shaping are
//! `octo_core::last_fm::last_fm_radio_recommendation_service`.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use indexmap::IndexMap;
use octo_core::common::{Clock, dotnet};
use octo_core::last_fm::SimilarTrack;
use octo_core::last_fm::last_fm_radio_recommendation_service::{
    ARTIST_RADIO_NEIGHBOURS, Candidate, FRONTIER_ARTIST_CAP, FRONTIER_ARTISTS, FRONTIER_FEWEST_SONGS,
    FRONTIER_NEIGHBOURS, FRONTIER_SEEDS, FRONTIER_TRACKS_EACH, FrontierScores, HEARTED_SEED_AFFINITY,
    IgnoreCaseKeys, NEIGHBOUR_ARTIST_AFFINITY, Randomizer, SECOND_STEP_SEEDS, SECOND_STEP_WEIGHT,
    SEED_RECENCY_DECAY, SEEDS_AT_ONCE, artist_cap, artist_key, create, default_randomizer,
    definition_version, discount_known, key, known_artists, ranked, score_artists, score_local_tags,
    seed_score, shape, shape_walk, shape_with_reserve, source_weight, suppress_station_overlap, title,
    to_candidate, unknown_neighbours_first,
};
use octo_core::last_fm::last_fm_radio_seed_normalizer as seed_normalizer;
use octo_core::last_fm::last_fm_radio_state_store::user_key;
use octo_core::library::generated_playlist_service::dotnet_ticks;
use octo_core::models::domain::Song;
use octo_core::models::radio::{
    LastFmRadioPlay, LastFmRadioStation, LastFmRadioStationKind, LastFmRadioTrack, LastFmRadioUserState,
};
use octo_core::radio::RadioSeed;
use octo_core::radio::radio_blend;
use octo_core::radio::radio_provider::LAST_FM;
use octo_core::settings::SettingsStore;
use parking_lot::Mutex;
use tokio::sync::Semaphore;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::last_fm_radio_state_store::LastFmRadioStateStore;
use super::{LastFmService, OperationCanceled};
use crate::services::radio::RadioSourceSet;

/// Provider expansion has a hard fan-out and deadline. Partial results are useful.
const PROVIDER_BUDGET: Duration = Duration::from_secs(25);

/// The similar-artist walk of Discovery Radio has this long of its own, within the build's budget.
const DISCOVERY_WALK_BUDGET: Duration = Duration::from_secs(8);

/// How many songs each source is asked for when a station goes on past its snapshot.
const CONTINUATION_CANDIDATES: i32 = 100;

/// How many stations' new songs are kept at once; the one walked on least recently goes first.
const CONTINUATIONS_KEPT: usize = 64;

/// How many new songs one station keeps; the oldest go first.
const CONTINUATION_SONGS: usize = 500;

/// How many of the newest songs left out seed a walk on from songs like them.
const CONTINUATION_SEEDS: usize = 3;

/// How many served songs are remembered across listeners; the oldest go first.
const SERVED_KEPT: usize = 5000;

/// How long a song a station served stays out of that station's songs.
pub const STATION_MEMORY: TimeDelta = TimeDelta::hours(24);

/// How many served songs one station remembers; the oldest go first.
pub const STATION_MEMORY_SONGS: usize = 1000;

/// How many stations' served songs are remembered at once; the one served least recently goes
/// first.
const STATIONS_REMEMBERED: usize = 256;

/// A song a station served, and when.
#[derive(Debug, Clone)]
struct StationServed {
    /// The song's key, folded for comparing.
    key: String,
    track: LastFmRadioTrack,
    at: chrono::DateTime<Utc>,
}

/// The caller's token with a 25 s deadline on top: either ends the build.
struct Budget<'a> {
    caller: &'a CancellationToken,
    deadline: Instant,
}

impl<'a> Budget<'a> {
    /// This budget cut to at most `limit` from now: the same caller, the earlier deadline.
    fn within(&self, limit: Duration) -> Budget<'a> {
        Budget {
            caller: self.caller,
            deadline: self.deadline.min(Instant::now() + limit),
        }
    }

    /// The call, or [`OperationCanceled`] once the caller cancels or the deadline passes. (A
    /// canceled call is dropped, which cancels its request.)
    async fn run<F: Future>(&self, call: F) -> Result<F::Output, OperationCanceled> {
        tokio::select! {
            biased;
            () = self.caller.cancelled() => Err(OperationCanceled),
            () = tokio::time::sleep_until(self.deadline) => Err(OperationCanceled),
            output = call => Ok(output),
        }
    }
}

/// Builds canonical station snapshots from bounded listening signals. Which candidates make the
/// cut is a weighted draw rather than a fixed top-N, so two refreshes of the same profile give
/// two different stations; the previous snapshot is demoted so a refresh rotates the station
/// instead of restating it.
pub struct LastFmRadioRecommendationService {
    last_fm: Arc<LastFmService>,
    state: Arc<LastFmRadioStateStore>,
    /// The Last.fm settings.
    settings: Arc<SettingsStore>,
    /// Source of the draw. Tests replace it with a seeded one so a build repeats exactly.
    randomizer: Mutex<Randomizer>,
    /// Every radio source; None keeps stations on Last.fm alone.
    sources: Option<Arc<RadioSourceSet>>,
    /// The new songs each station went on with, by listener, station and snapshot, the station
    /// walked on most recently last.
    continuations: Mutex<IndexMap<String, Vec<LastFmRadioTrack>>>,
    /// The songs stations served lately, by listener and song id, the oldest first.
    served: Mutex<IndexMap<String, LastFmRadioTrack>>,
    /// What each station served a listener through getStationSongs and its stream, by listener
    /// and station, the station served least recently first; each list oldest first.
    station_history: Mutex<IndexMap<String, VecDeque<StationServed>>>,
    /// The clock the station history is kept by. Tests move it.
    clock: Mutex<Clock>,
}

impl LastFmRadioRecommendationService {
    pub fn new(
        last_fm: Arc<LastFmService>,
        state: Arc<LastFmRadioStateStore>,
        settings: Arc<SettingsStore>,
        sources: Option<Arc<RadioSourceSet>>,
    ) -> Self {
        LastFmRadioRecommendationService {
            last_fm,
            state,
            settings,
            randomizer: Mutex::new(default_randomizer()),
            sources,
            continuations: Mutex::new(IndexMap::new()),
            served: Mutex::new(IndexMap::new()),
            station_history: Mutex::new(IndexMap::new()),
            clock: Mutex::new(Clock::system()),
        }
    }

    /// `Randomizer = ...`.
    pub fn set_randomizer(&self, randomizer: Randomizer) {
        *self.randomizer.lock() = randomizer;
    }

    /// The clock the station history is kept by.
    pub fn set_clock(&self, clock: Clock) {
        *self.clock.lock() = clock;
    }

    /// The listener's stations, built now. An error is an answer Last.fm's reader refused, or
    /// [`OperationCanceled`] when the caller canceled or the budget ran out while an artist
    /// station was being built.
    pub async fn build(
        &self,
        username: &str,
        cancellation_token: &CancellationToken,
    ) -> anyhow::Result<Vec<LastFmRadioStation>> {
        let snapshot = self.settings.current();
        let settings = &snapshot.last_fm;
        if !settings.enable_radio {
            return Ok(Vec::new());
        }
        let user = self.state.get_user(username);
        let now = Utc::now();
        let randomizer = self.randomizer.lock().clone();
        let mut random = randomizer();
        // Grouped by key ignoring case: the first station with a key names its snapshot.
        let mut previous_snapshots: Vec<(String, IgnoreCaseKeys)> = Vec::new();
        for station in &user.stations {
            if !previous_snapshots
                .iter()
                .any(|(key, _)| dotnet::eq_ignore_case(key, &station.key))
            {
                let keys = station
                    .tracks
                    .iter()
                    .map(|track| seed_normalizer::track_key(&track.artist, &track.title))
                    .collect();
                previous_snapshots.push((station.key.clone(), keys));
            }
        }
        let empty = IgnoreCaseKeys::new();
        let previous = |station_key: &str| previous_snapshot(&previous_snapshots, &empty, station_key);
        let Signals {
            plays,
            unavailable,
            banned,
            disliked,
        } = Signals::of(&user);
        let known = known_artists(&plays);
        let refill_headroom = (unavailable.len() as i32).min(settings.effective_radio_track_count());
        let candidate_target = 100.min(settings.effective_radio_track_count() + refill_headroom + 10);
        let artist_scores = score_artists(&plays, now);
        let track_seeds = track_seeds(&plays, now);
        let mut tags = score_local_tags(&plays);

        // Provider expansion has a hard fan-out and deadline. Partial results are useful.
        let budget = Budget {
            caller: cancellation_token,
            deadline: Instant::now() + PROVIDER_BUDGET,
        };
        for (artist, _) in artist_scores.iter().take(5) {
            match budget.run(self.last_fm.get_artist_top_tags(artist, 6)).await {
                Ok(top_tags) => {
                    for tag in top_tags? {
                        tags.add(&tag, 1.0);
                    }
                }
                Err(canceled) if cancellation_token.is_cancelled() => return Err(canceled.into()),
                Err(_) => break,
            }
        }

        let mut stations: Vec<LastFmRadioStation> = Vec::new();
        if settings.enable_personalized_stations {
            let learned: f64 = plays
                .iter()
                .filter(|play| play.learned_signal)
                .map(source_weight)
                .sum();
            let learned = learned >= f64::from(settings.effective_minimum_plays());
            let mix_key = if learned { "your-mix" } else { "starter" };
            if settings.enable_your_mix {
                let mut mix_candidates = self
                    .tracks_from_seeds(username, &track_seeds[..track_seeds.len().min(6)], 12, &budget)
                    .await;
                let familiar: Vec<Candidate> = plays
                    .iter()
                    .enumerate()
                    .map(|(rank, play)| to_candidate(play, rank))
                    .collect();
                // The familiar share of the mix is a quota the walk enforces, so both halves are
                // drawn over their whole pools rather than the top of each list.
                let familiar_quota = if mix_candidates.is_empty() {
                    None
                } else {
                    Some(familiar_quota(settings))
                };
                if mix_candidates.is_empty() {
                    mix_candidates.extend(familiar.iter().cloned());
                }
                let seeds: Vec<&str> = track_seeds.iter().map(|seed| seed.artist.as_str()).collect();
                let tracks = shape(
                    familiar.into_iter().chain(mix_candidates).collect(),
                    &plays,
                    settings,
                    &unavailable,
                    &mut *random,
                    previous(mix_key),
                    artist_cap(settings, LastFmRadioStationKind::YourMix),
                    false,
                    familiar_quota,
                    Some(&banned),
                    Some(&disliked),
                );
                stations.push(create(
                    username,
                    mix_key,
                    if learned { "Your Radio" } else { "Starter Radio" },
                    if learned {
                        LastFmRadioStationKind::YourMix
                    } else {
                        LastFmRadioStationKind::Starter
                    },
                    true,
                    &seeds,
                    tracks,
                    1,
                    Utc::now(),
                ));
            }

            if learned {
                let top_tags = tags.top_by_score_then_name(3);
                // The artist and genre stations' candidates are asked for first, so a slow walk
                // for Discovery Radio never spends their time; the stations are still drawn and
                // listed in the same order as before.
                let mut artist_candidates: Vec<(String, Vec<Candidate>)> = Vec::new();
                for (artist, _) in artist_scores
                    .iter()
                    .take(settings.effective_artist_station_count().max(0) as usize)
                {
                    let candidates = self
                        .tracks_from_artist(username, artist, candidate_target, &known, &budget)
                        .await?;
                    artist_candidates.push((artist.clone(), candidates));
                }
                let mut genre_candidates: Vec<(String, Vec<Candidate>)> = Vec::new();
                for tag in tags.top_by_score(settings.effective_genre_station_count().max(0) as usize) {
                    // A genre station is for finding: the listener's own artists keep a third of
                    // their weight, so the genre's other names lead.
                    let candidates = discount_known(
                        self.tracks_from_tags(
                            username,
                            std::slice::from_ref(&tag),
                            candidate_target,
                            &budget,
                        )
                        .await?,
                        &known,
                    );
                    genre_candidates.push((tag, candidates));
                }

                if settings.enable_discovery_mix {
                    // New artists first, found through the ones the listener plays, within the
                    // walk's own few seconds; the tag charts only when the walk finds too few
                    // songs.
                    let walk_budget = budget.within(DISCOVERY_WALK_BUDGET);
                    let mut discovery = self
                        .tracks_from_frontier(&artist_scores, &known, &disliked, &walk_budget)
                        .await;
                    let mut discovery_cap = FRONTIER_ARTIST_CAP;
                    if discovery.len() < FRONTIER_FEWEST_SONGS && !top_tags.is_empty() {
                        discovery = self
                            .tracks_from_tags(username, &top_tags, candidate_target, &budget)
                            .await?;
                        discovery_cap = artist_cap(settings, LastFmRadioStationKind::Discovery);
                    }
                    if discovery.len() >= 5 {
                        // The reserve stands in for library songs past the Discovery balance's
                        // share, which only resolving the tracks can tell.
                        let shaped = shape_with_reserve(
                            discovery,
                            &plays,
                            settings,
                            &unavailable,
                            &mut *random,
                            previous("discovery"),
                            discovery_cap,
                            true,
                            Some(&banned),
                            Some(&disliked),
                        );
                        let mut station = create(
                            username,
                            "discovery",
                            "Discovery Radio",
                            LastFmRadioStationKind::Discovery,
                            true,
                            &top_tags,
                            shaped.tracks,
                            1,
                            Utc::now(),
                        );
                        station.reserve = shaped.reserve;
                        stations.push(station);
                    }
                }

                for (artist, candidates) in artist_candidates {
                    let station_key = format!("artist-{}", key(&artist));
                    if candidates.len() >= 5 {
                        let tracks = shape(
                            candidates,
                            &plays,
                            settings,
                            &unavailable,
                            &mut *random,
                            previous(&station_key),
                            artist_cap(settings, LastFmRadioStationKind::Artist),
                            false,
                            None,
                            Some(&banned),
                            Some(&disliked),
                        );
                        stations.push(create(
                            username,
                            &station_key,
                            &format!("{artist} Radio"),
                            LastFmRadioStationKind::Artist,
                            true,
                            &[artist.as_str()],
                            tracks,
                            1,
                            Utc::now(),
                        ));
                    }
                }

                for (tag, candidates) in genre_candidates {
                    let station_key = format!("genre-{}", key(&tag));
                    let artists: IgnoreCaseKeys =
                        candidates.iter().map(|item| item.track.artist.as_str()).collect();
                    if artists.len() >= 4 {
                        let tracks = shape(
                            candidates,
                            &plays,
                            settings,
                            &unavailable,
                            &mut *random,
                            previous(&station_key),
                            artist_cap(settings, LastFmRadioStationKind::Genre),
                            false,
                            None,
                            Some(&banned),
                            Some(&disliked),
                        );
                        stations.push(create(
                            username,
                            &station_key,
                            &format!("{} Radio", title(&tag)),
                            LastFmRadioStationKind::Genre,
                            true,
                            &[tag.as_str()],
                            tracks,
                            1,
                            Utc::now(),
                        ));
                    }
                }
            }
        }

        if settings.enable_discovery_stations {
            for definition in settings
                .effective_discovery_stations()
                .into_iter()
                .filter(|item| item.enabled)
            {
                let mut candidates = self
                    .tracks_from_tags(username, &definition.tags, candidate_target, &budget)
                    .await?;
                if candidates.is_empty() {
                    candidates.extend(
                        plays
                            .iter()
                            .filter(|play| {
                                definition.tags.iter().any(|tag| {
                                    contains_ignore_case(play.genre.as_deref().unwrap_or_default(), tag)
                                })
                            })
                            .enumerate()
                            .map(|(rank, play)| to_candidate(play, rank)),
                    );
                }
                let station_key = format!("pinned-{}", definition.id);
                if !candidates.is_empty() {
                    let tracks = shape(
                        candidates,
                        &plays,
                        settings,
                        &unavailable,
                        &mut *random,
                        previous(&station_key),
                        artist_cap(settings, LastFmRadioStationKind::Pinned),
                        false,
                        None,
                        Some(&banned),
                        Some(&disliked),
                    );
                    stations.push(create(
                        username,
                        &station_key,
                        &definition.name,
                        LastFmRadioStationKind::Pinned,
                        false,
                        &definition.tags,
                        tracks,
                        definition_version(&definition),
                        Utc::now(),
                    ));
                }
            }
        }

        suppress_station_overlap(&mut stations);
        for station in &mut stations {
            station.valid_until_utc = station.changed_utc
                + TimeDelta::hours(i64::from(settings.effective_refresh_interval_hours()));
        }
        info!("Built {} Last.fm radio stations for {username}", stations.len());
        Ok(stations
            .into_iter()
            .filter(|station| !station.tracks.is_empty())
            .collect())
    }

    /// Up to `count` songs for the listener's station `station_id`, none of them in `exclude`
    /// (matched by artist and title) and none the station served the listener in the last
    /// [`STATION_MEMORY`]: first the station's own tracks and then its reserve, in their order,
    /// then new songs of the station's kind. The new songs are drawn and shaped as a build draws
    /// the station, with everything the station holds and everything left out counted as
    /// already in it, and are kept for the station's snapshot, so Last.fm is asked again only
    /// once those run out. Once nothing new is found, the songs served longest ago come round
    /// again (never one in `exclude`). Empty while radio is off and for a station the listener
    /// does not have; Last.fm is waited on no longer than a build waits.
    pub async fn next_for_station(
        &self,
        username: &str,
        station_id: &str,
        exclude: &[LastFmRadioTrack],
        count: usize,
        cancellation_token: &CancellationToken,
    ) -> Vec<LastFmRadioTrack> {
        self.pick_for_station(username, station_id, exclude, count, true, cancellation_token)
            .await
    }

    /// [`Self::next_for_station`] without the songs served longest ago coming round again: empty
    /// once nothing new is found.
    pub async fn next_new_for_station(
        &self,
        username: &str,
        station_id: &str,
        exclude: &[LastFmRadioTrack],
        count: usize,
        cancellation_token: &CancellationToken,
    ) -> Vec<LastFmRadioTrack> {
        self.pick_for_station(username, station_id, exclude, count, false, cancellation_token)
            .await
    }

    /// The picks behind [`Self::next_for_station`]; with `fall_back`, the songs served longest ago
    /// when nothing new is found.
    async fn pick_for_station(
        &self,
        username: &str,
        station_id: &str,
        exclude: &[LastFmRadioTrack],
        count: usize,
        fall_back: bool,
        cancellation_token: &CancellationToken,
    ) -> Vec<LastFmRadioTrack> {
        if count == 0 || !self.settings.current().last_fm.enable_radio {
            return Vec::new();
        }
        let Some(station) = self.state.find_station(username, station_id) else {
            return Vec::new();
        };
        // What the station served in the last day stays out too, as if the caller had sent it.
        let remembered = self.station_served(username, station_id);
        let asked_out: IgnoreCaseKeys = exclude.iter().map(track_key).collect();
        let left_out: IgnoreCaseKeys = exclude.iter().chain(&remembered).map(track_key).collect();
        let exclude: Vec<LastFmRadioTrack> = remembered.iter().chain(exclude).cloned().collect();
        let key = continuation_key(username, &station);
        let kept = self.continuations.lock().get(&key).cloned().unwrap_or_default();
        let mut picks = Vec::new();
        let mut taken = IgnoreCaseKeys::new();
        pick_fresh(
            &mut picks,
            &mut taken,
            &left_out,
            station.tracks.iter().chain(&station.reserve).chain(&kept),
            count,
        );
        if picks.len() >= count {
            return picks;
        }
        let walked = self
            .walk_on(username, &station, &exclude, &kept, cancellation_token)
            .await;
        if !walked.is_empty() {
            self.keep_walked(key, &walked);
        }
        pick_fresh(&mut picks, &mut taken, &left_out, &walked, count);
        if picks.is_empty() && fall_back {
            // Nothing new anywhere: the songs served longest ago come round again rather than none.
            pick_fresh(&mut picks, &mut taken, &asked_out, &remembered, count);
        }
        picks
    }

    /// Remembers that the listener's station served these songs now, so for
    /// [`STATION_MEMORY`] it serves none of them again (at most [`STATION_MEMORY_SONGS`] a
    /// station, the oldest forgotten first).
    pub fn record_station_served(
        &self,
        username: &str,
        station_id: &str,
        tracks: impl IntoIterator<Item = LastFmRadioTrack>,
    ) {
        let now = self.clock.lock().now();
        let key = history_key(username, station_id);
        let mut history = self.station_history.lock();
        let mut served = history.shift_remove(&key).unwrap_or_default();
        for track in tracks {
            let song = dotnet::ordinal_ignore_case_key(&track_key(&track));
            served.retain(|earlier| earlier.key != song);
            served.push_back(StationServed {
                key: song,
                track,
                at: now,
            });
        }
        while served.len() > STATION_MEMORY_SONGS {
            served.pop_front();
        }
        history.insert(key, served);
        while history.len() > STATIONS_REMEMBERED {
            history.shift_remove_index(0);
        }
    }

    /// The songs the listener's station served in the last [`STATION_MEMORY`], oldest first.
    pub fn station_served(&self, username: &str, station_id: &str) -> Vec<LastFmRadioTrack> {
        let since = self.clock.lock().now() - STATION_MEMORY;
        self.station_history
            .lock()
            .get(&history_key(username, station_id))
            .map(|served| {
                served
                    .iter()
                    .filter(|entry| entry.at > since)
                    .map(|entry| entry.track.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// New songs of the station's kind, none of them already in the station, in `kept` or in
    /// `exclude`. The station's own seeds are asked first (its artist, its tags, the listener's
    /// strongest artists or their recent songs); once those have nothing left, songs like the
    /// newest ones left out stand in, or like the station's last tracks when nothing was left
    /// out.
    async fn walk_on(
        &self,
        username: &str,
        station: &LastFmRadioStation,
        exclude: &[LastFmRadioTrack],
        kept: &[LastFmRadioTrack],
        cancellation_token: &CancellationToken,
    ) -> Vec<LastFmRadioTrack> {
        let snapshot = self.settings.current();
        let settings = &snapshot.last_fm;
        let user = self.state.get_user(username);
        let now = Utc::now();
        let Signals {
            plays,
            mut unavailable,
            banned,
            disliked,
        } = Signals::of(&user);
        for track in station
            .tracks
            .iter()
            .chain(&station.reserve)
            .chain(kept)
            .chain(exclude)
        {
            unavailable.insert(&track_key(track));
        }
        let previous: IgnoreCaseKeys = station.tracks.iter().chain(exclude).map(track_key).collect();
        let known = known_artists(&plays);
        let budget = Budget {
            caller: cancellation_token,
            deadline: Instant::now() + PROVIDER_BUDGET,
        };
        let newest: Vec<&LastFmRadioTrack> = if exclude.is_empty() {
            station.tracks.iter().rev().take(CONTINUATION_SEEDS).collect()
        } else {
            exclude.iter().rev().take(CONTINUATION_SEEDS).collect()
        };
        let newest: Vec<LastFmRadioPlay> = newest.into_iter().map(as_seed).collect();

        let kind = station.kind;
        let mut cap = artist_cap(settings, kind);
        let mut exclude_recent = false;
        let mut quota = None;
        let candidates = match kind {
            LastFmRadioStationKind::Artist => match station.seeds.first() {
                Some(artist) => self
                    .tracks_from_artist(username, artist, CONTINUATION_CANDIDATES, &known, &budget)
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            },
            LastFmRadioStationKind::Genre => discount_known(
                self.tracks_from_tags(username, &station.seeds, CONTINUATION_CANDIDATES, &budget)
                    .await
                    .unwrap_or_default(),
                &known,
            ),
            LastFmRadioStationKind::Pinned => self
                .tracks_from_tags(username, &station.seeds, CONTINUATION_CANDIDATES, &budget)
                .await
                .unwrap_or_default(),
            LastFmRadioStationKind::Discovery => {
                exclude_recent = true;
                cap = FRONTIER_ARTIST_CAP;
                let artist_scores = score_artists(&plays, now);
                let walk_budget = budget.within(DISCOVERY_WALK_BUDGET);
                let mut found = self
                    .tracks_from_frontier(&artist_scores, &known, &disliked, &walk_budget)
                    .await;
                if found.len() < FRONTIER_FEWEST_SONGS && !station.seeds.is_empty() {
                    found = self
                        .tracks_from_tags(username, &station.seeds, CONTINUATION_CANDIDATES, &budget)
                        .await
                        .unwrap_or_default();
                    cap = artist_cap(settings, kind);
                }
                found
            }
            LastFmRadioStationKind::YourMix | LastFmRadioStationKind::Starter => {
                // The newest songs left out lead the seeds, so the mix follows where it went.
                let mut seen = IgnoreCaseKeys::new();
                let seeds: Vec<LastFmRadioPlay> = newest
                    .iter()
                    .cloned()
                    .chain(track_seeds(&plays, now))
                    .filter(|seed| seen.insert(&seed_normalizer::track_key(&seed.artist, &seed.title)))
                    .take(6)
                    .collect();
                let mut mix = self.tracks_from_seeds(username, &seeds, 12, &budget).await;
                let familiar: Vec<Candidate> = plays
                    .iter()
                    .enumerate()
                    .map(|(rank, play)| to_candidate(play, rank))
                    .collect();
                if mix.is_empty() {
                    mix.extend(familiar.iter().cloned());
                } else {
                    quota = Some(familiar_quota(settings));
                }
                familiar.into_iter().chain(mix).collect()
            }
        };
        let randomizer = self.randomizer.lock().clone();
        let mut random = randomizer();
        let mut walked = shape_walk(
            candidates,
            &plays,
            settings,
            &unavailable,
            &mut *random,
            &previous,
            cap,
            exclude_recent,
            quota,
            Some(&banned),
            Some(&disliked),
            1,
        )
        .tracks;
        let seeded_from_newest = matches!(
            kind,
            LastFmRadioStationKind::YourMix | LastFmRadioStationKind::Starter
        );
        if walked.is_empty() && !seeded_from_newest {
            let mut like = self.tracks_from_seeds(username, &newest, 12, &budget).await;
            if matches!(
                kind,
                LastFmRadioStationKind::Discovery | LastFmRadioStationKind::Genre
            ) {
                like = discount_known(like, &known);
            }
            walked = shape_walk(
                like,
                &plays,
                settings,
                &unavailable,
                &mut *random,
                &previous,
                artist_cap(settings, kind),
                exclude_recent,
                None,
                Some(&banned),
                Some(&disliked),
                1,
            )
            .tracks;
        }
        walked
    }

    /// Adds `walked` to what the station keeps for its snapshot, and makes it the station walked
    /// on most recently.
    fn keep_walked(&self, key: String, walked: &[LastFmRadioTrack]) {
        let mut continuations = self.continuations.lock();
        let mut kept = continuations.shift_remove(&key).unwrap_or_default();
        let mut known: IgnoreCaseKeys = kept.iter().map(track_key).collect();
        kept.extend(
            walked
                .iter()
                .filter(|track| known.insert(&track_key(track)))
                .cloned(),
        );
        if kept.len() > CONTINUATION_SONGS {
            kept.drain(..kept.len() - CONTINUATION_SONGS);
        }
        continuations.insert(key, kept);
        let over = continuations.len().saturating_sub(CONTINUATIONS_KEPT);
        if over > 0 {
            continuations.drain(..over);
        }
    }

    /// Remembers which song each served id stands for, so a later ask that leaves the id out
    /// leaves the song out.
    pub fn remember_served<'a>(&self, username: &str, songs: impl IntoIterator<Item = &'a Song>) {
        let mut served = self.served.lock();
        for song in songs.into_iter().filter(|song| !song.id.is_empty()) {
            served.insert(
                served_key(username, &song.id),
                LastFmRadioTrack {
                    artist: song.artist.clone(),
                    title: song.title.clone(),
                    album: (!dotnet::is_blank(&song.album)).then(|| song.album.clone()),
                    genre: song.genre.clone(),
                    duration: song.duration,
                    resolved_id: Some(song.id.clone()),
                    is_local: song.is_local,
                    ..Default::default()
                },
            );
        }
        let over = served.len().saturating_sub(SERVED_KEPT);
        if over > 0 {
            served.drain(..over);
        }
    }

    /// The song a served id stands for, when one was remembered for this listener.
    pub fn served_track(&self, username: &str, id: &str) -> Option<LastFmRadioTrack> {
        self.served.lock().get(&served_key(username, id)).cloned()
    }

    /// Tracks similar to each seed, the seed's own weight riding along: the newest seed leads,
    /// each older one contributes [`SEED_RECENCY_DECAY`] of the previous, and a hearted seed
    /// counts half again. With Last.fm alone, its match score stays the per-track signal; with
    /// several sources, each source's list is ranked by position and weighted by its provider
    /// (see [`RadioSourceSet::weights`]), and seeds are asked [`SEEDS_AT_ONCE`] at a time so a
    /// slow source costs one timeout per three seeds.
    async fn tracks_from_seeds(
        &self,
        username: &str,
        seeds: &[LastFmRadioPlay],
        each: i32,
        budget: &Budget<'_>,
    ) -> Vec<Candidate> {
        let ordered: Vec<(&LastFmRadioPlay, f64)> = seeds
            .iter()
            .enumerate()
            .map(|(position, seed)| {
                (
                    seed,
                    SEED_RECENCY_DECAY.powi(position as i32)
                        * if seed.hearted { HEARTED_SEED_AFFINITY } else { 1.0 },
                )
            })
            .collect();
        let Some(sources) = &self.sources else {
            let mut result = Vec::new();
            for (seed, affinity) in ordered {
                let source = seed_normalizer::track_key(&seed.artist, &seed.title);
                match budget
                    .run(self.last_fm.get_similar_tracks(&seed.artist, &seed.title, each))
                    .await
                {
                    Ok(tracks) => result.extend(ranked(tracks, &source, affinity)),
                    Err(_) => break,
                }
            }
            return result;
        };

        let weights = sources.weights(Some(username), false);
        let gate = Semaphore::new(SEEDS_AT_ONCE);
        let empty = IndexMap::new();
        let lists = futures::future::join_all(ordered.into_iter().map(|(seed, affinity)| {
            let (gate, weights, empty) = (&gate, &weights, &empty);
            async move {
                let Ok(_permit) = budget.run(gate.acquire()).await else {
                    return Vec::new();
                };
                let source = seed_normalizer::track_key(&seed.artist, &seed.title);
                let library_song = (seed.is_local && !seed.song_id.is_empty()).then(|| Song {
                    id: seed.song_id.clone(),
                    title: seed.title.clone(),
                    artist: seed.artist.clone(),
                    is_local: true,
                    ..Default::default()
                });
                let radio_seed = RadioSeed::new(
                    seed.artist.clone(),
                    seed.title.clone(),
                    seed.duration,
                    library_song,
                );
                let Ok(answers) = budget.run(sources.ask_all(&radio_seed, each, empty, None)).await else {
                    return Vec::new();
                };
                let mut found = Vec::new();
                for answer in answers {
                    let factor = radio_blend::station_factor(answer.r#match)
                        * radio_blend::thin(&answer, each)
                        * weights.get(&answer.provider).copied().unwrap_or(1.0);
                    if factor <= 0.0 {
                        continue;
                    }
                    // Sources score on different scales (Last.fm's match, a flat 1 from YouTube
                    // Music), so with several sources each list is ranked by position alone, as
                    // song radio does, and the provider weights say how much each list counts.
                    let tracks: Vec<SimilarTrack> = answer
                        .library_songs
                        .iter()
                        .map(|song| {
                            SimilarTrack::new(song.artist.clone(), song.title.clone(), 1.0)
                                .with_duration(song.duration)
                                .with_provider(Some(answer.provider.clone()))
                        })
                        .chain(answer.tracks.iter().map(|track| SimilarTrack {
                            r#match: 1.0,
                            ..track.clone()
                        }))
                        .collect();
                    found.extend(ranked(
                        tracks,
                        &format!("{}:{source}", answer.provider),
                        affinity * factor,
                    ));
                }
                found
            }
        }))
        .await;
        lists.into_iter().flatten().collect()
    }

    async fn tracks_from_tags(
        &self,
        username: &str,
        tags: &[String],
        each: i32,
        budget: &Budget<'_>,
    ) -> anyhow::Result<Vec<Candidate>> {
        let mut result = Vec::new();
        for tag in tags.iter().take(5) {
            match budget.run(self.last_fm.get_tag_top_tracks(tag, each)).await {
                Ok(tracks) => result.extend(ranked(tracks?, &format!("tag:{tag}"), 1.0)),
                Err(_) => break,
            }
            // Last.fm's tag chart is the line above; the other sources' answers for the tag (LB
            // Radio, with a ListenBrainz token) join it, each list weighted by its provider.
            if let Some(sources) = &self.sources {
                let weights = sources.weights(Some(username), false);
                match budget.run(sources.tag_all(tag, each)).await {
                    Ok(answers) => {
                        for answer in answers.into_iter().filter(|answer| answer.provider != LAST_FM) {
                            let weight = weights.get(&answer.provider).copied().unwrap_or(1.0);
                            let source = format!("{}:tag:{tag}", answer.provider);
                            result.extend(ranked(answer.tracks, &source, weight));
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        Ok(result)
    }

    /// The seed artist's own top tracks lead; similar artists' top tracks ride at
    /// [`NEIGHBOUR_ARTIST_AFFINITY`] so the station stays about who it is named for. Of
    /// [`ARTIST_RADIO_NEIGHBOURS`] neighbours, the five used are the strongest with the ones the
    /// listener knows counted at a third of their match: a small artist whose closest neighbours
    /// are all familiar still leads somewhere new.
    async fn tracks_from_artist(
        &self,
        username: &str,
        artist: &str,
        candidate_target: i32,
        known: &IgnoreCaseKeys,
        budget: &Budget<'_>,
    ) -> anyhow::Result<Vec<Candidate>> {
        let mut result = Vec::new();
        // The build's budget running out keeps what was found so far: the artist's own top
        // tracks alone still make a station.
        let Ok(own) = budget
            .run(
                self.last_fm
                    .get_artist_top_tracks(artist, 50.min(candidate_target)),
            )
            .await
        else {
            return Ok(result);
        };
        result.extend(ranked(own, &format!("artist:{artist}"), 1.0));
        let Ok(similar) = budget
            .run(self.last_fm.get_similar_artists(artist, ARTIST_RADIO_NEIGHBOURS))
            .await
        else {
            return Ok(result);
        };
        for similar in unknown_neighbours_first(similar?, known).into_iter().take(5) {
            let Ok(tracks) = budget
                .run(
                    self.last_fm
                        .get_artist_top_tracks(&similar.name, 20.min(6.max(candidate_target / 5))),
                )
                .await
            else {
                return Ok(result);
            };
            result.extend(ranked(
                tracks,
                &format!("artist:{}", similar.name),
                NEIGHBOUR_ARTIST_AFFINITY,
            ));
        }
        // The other sources' radio for the artist (YouTube Music's, LB Radio's), beside the
        // similar artists' songs. Last.fm is not asked again: its answer is the lines above.
        if let Some(sources) = &self.sources {
            let weights = sources.weights(Some(username), false);
            let seed = RadioSeed::new(artist, "", None, None);
            let empty = IndexMap::new();
            let not_last_fm =
                |source: &dyn crate::services::radio::IRadioSource| source.provider() != LAST_FM;
            let Ok(answers) = budget
                .run(sources.ask_all(&seed, candidate_target, &empty, Some(&not_last_fm)))
                .await
            else {
                return Ok(result);
            };
            for answer in answers.into_iter().filter(|answer| answer.provider != LAST_FM) {
                let weight = weights.get(&answer.provider).copied().unwrap_or(1.0);
                let source = format!("{}:artist:{artist}", answer.provider);
                result.extend(ranked(answer.tracks, &source, NEIGHBOUR_ARTIST_AFFINITY * weight));
            }
        }
        Ok(result)
    }

    /// Songs by artists the listener does not know yet, found through the ones they play (see
    /// [`FRONTIER_SEEDS`]): the walk scores each new neighbour of the strongest artists, steps
    /// once more from the best of them, then asks the best new artists for their top songs, each
    /// list weighted by its artist's score. The build's budget running out keeps what the walk
    /// has found so far.
    async fn tracks_from_frontier(
        &self,
        artist_scores: &[(String, f64)],
        known: &IgnoreCaseKeys,
        disliked: &IgnoreCaseKeys,
        budget: &Budget<'_>,
    ) -> Vec<Candidate> {
        let seeds: Vec<&(String, f64)> = artist_scores.iter().take(FRONTIER_SEEDS).collect();
        let Some(strongest) = seeds.first().map(|(_, score)| score.max(1e-9)) else {
            return Vec::new();
        };
        let mut scores = FrontierScores::default();
        let first: Vec<(String, f64)> = seeds
            .iter()
            .map(|(artist, score)| (artist.clone(), score / strongest))
            .collect();
        for (similar, weight) in self.neighbours(&first, budget).await {
            scores.add(&similar, weight, known, disliked);
        }
        let step = scores.top(SECOND_STEP_SEEDS);
        if let Some(best) = step.first().map(|(_, score)| score.max(1e-9)) {
            let from: Vec<(String, f64)> = step
                .iter()
                .map(|(artist, score)| (artist.clone(), SECOND_STEP_WEIGHT * score / best))
                .collect();
            for (similar, weight) in self.neighbours(&from, budget).await {
                scores.add(&similar, weight, known, disliked);
            }
        }
        let picked = scores.top(FRONTIER_ARTISTS);
        let Some(top) = picked.first().map(|(_, score)| score.max(1e-9)) else {
            return Vec::new();
        };
        let gate = Semaphore::new(SEEDS_AT_ONCE);
        let lists = futures::future::join_all(picked.iter().map(|(artist, score)| {
            let gate = &gate;
            async move {
                let Ok(_permit) = budget.run(gate.acquire()).await else {
                    return Vec::new();
                };
                let Ok(tracks) = budget
                    .run(self.last_fm.get_artist_top_tracks(artist, FRONTIER_TRACKS_EACH))
                    .await
                else {
                    return Vec::new();
                };
                ranked(tracks, &format!("frontier:{artist}"), score / top)
            }
        }))
        .await;
        lists.into_iter().flatten().collect()
    }

    /// Each artist's similar artists with the weight its answer carries; an artist whose
    /// answer did not come in time, or could not be read, adds nothing.
    async fn neighbours(
        &self,
        from: &[(String, f64)],
        budget: &Budget<'_>,
    ) -> Vec<(Vec<octo_core::last_fm::SimilarArtist>, f64)> {
        futures::future::join_all(from.iter().map(|(artist, weight)| async move {
            match budget
                .run(self.last_fm.get_similar_artists(artist, FRONTIER_NEIGHBOURS))
                .await
            {
                Ok(Ok(similar)) => Some((similar, *weight)),
                _ => None,
            }
        }))
        .await
        .into_iter()
        .flatten()
        .collect()
    }
}

/// What a listener's state says about what may play: their plays newest first, the songs that
/// would not play lately, the songs they rated one star, and those songs' artists.
struct Signals {
    plays: Vec<LastFmRadioPlay>,
    unavailable: IgnoreCaseKeys,
    banned: IgnoreCaseKeys,
    disliked: IgnoreCaseKeys,
}

impl Signals {
    fn of(user: &LastFmRadioUserState) -> Self {
        let mut plays = user.plays.clone();
        plays.sort_by_key(|a| std::cmp::Reverse(a.played_at_utc));
        let unavailable: IgnoreCaseKeys = user
            .unavailable_tracks
            .iter()
            .filter(|track| track.retry_after_utc > Utc::now())
            .map(|track| track.key.as_str())
            .collect();
        let banned: IgnoreCaseKeys = user.radio_bans.iter().map(|ban| ban.key.as_str()).collect();
        let disliked: IgnoreCaseKeys = user
            .radio_bans
            .iter()
            .map(|ban| artist_key(&ban.artist))
            .collect();
        Signals {
            plays,
            unavailable,
            banned,
            disliked,
        }
    }
}

/// The eight strongest recent signals among `plays`, one per song. Seeds are not simply the
/// newest plays: a heart outranks a play, a chosen play outranks one the radio served, and age
/// decays.
fn track_seeds(plays: &[LastFmRadioPlay], now: chrono::DateTime<Utc>) -> Vec<LastFmRadioPlay> {
    let mut by_seed_score: Vec<&LastFmRadioPlay> = plays.iter().collect();
    by_seed_score.sort_by(|a, b| {
        seed_score(b, now)
            .partial_cmp(&seed_score(a, now))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut seen_seeds = std::collections::HashSet::new();
    by_seed_score
        .into_iter()
        .filter(|play| seen_seeds.insert(seed_normalizer::track_key(&play.artist, &play.title)))
        .take(8)
        .cloned()
        .collect()
}

/// How many of Your Radio's songs are the listener's own: what the Discovery balance leaves.
fn familiar_quota(settings: &octo_core::settings::LastFmSettings) -> i32 {
    let count = settings.effective_radio_track_count();
    count
        - dotnet::round(
            f64::from(count) * f64::from(settings.effective_discovery_percent()) / 100.0,
            0,
        ) as i32
}

/// A track's key, as the walk matches songs.
fn track_key(track: &LastFmRadioTrack) -> String {
    seed_normalizer::track_key(&track.artist, &track.title)
}

/// Adds the `tracks` not left out and not already taken to `picks`, in order, until it holds
/// `count`.
fn pick_fresh<'a>(
    picks: &mut Vec<LastFmRadioTrack>,
    taken: &mut IgnoreCaseKeys,
    left_out: &IgnoreCaseKeys,
    tracks: impl IntoIterator<Item = &'a LastFmRadioTrack>,
    count: usize,
) {
    for track in tracks {
        if picks.len() >= count {
            return;
        }
        let key = track_key(track);
        if left_out.contains(&key) || !taken.insert(&key) {
            continue;
        }
        picks.push(track.clone());
    }
}

/// Where a station's new songs are kept: its listener, its id and its snapshot, so a rebuilt
/// station starts afresh.
fn continuation_key(username: &str, station: &LastFmRadioStation) -> String {
    format!(
        "{}|{}|{}",
        dotnet::ordinal_ignore_case_key(&user_key(username)),
        station.id,
        dotnet_ticks(station.changed_utc)
    )
}

/// Where a station's served songs are remembered: its listener and its id.
fn history_key(username: &str, station_id: &str) -> String {
    format!(
        "{}|{station_id}",
        dotnet::ordinal_ignore_case_key(&user_key(username))
    )
}

/// Where a served song is remembered: its listener and its id.
fn served_key(username: &str, id: &str) -> String {
    format!("{}|{id}", dotnet::ordinal_ignore_case_key(&user_key(username)))
}

/// A track as a seed for songs like it.
fn as_seed(track: &LastFmRadioTrack) -> LastFmRadioPlay {
    LastFmRadioPlay {
        artist: track.artist.clone(),
        title: track.title.clone(),
        album: track.album.clone(),
        genre: track.genre.clone(),
        duration: track.duration,
        ..Default::default()
    }
}

/// The station's previous snapshot, matched by key ignoring case, or `empty`.
fn previous_snapshot<'a>(
    snapshots: &'a [(String, IgnoreCaseKeys)],
    empty: &'a IgnoreCaseKeys,
    station_key: &str,
) -> &'a IgnoreCaseKeys {
    snapshots
        .iter()
        .find(|(key, _)| dotnet::eq_ignore_case(key, station_key))
        .map_or(empty, |(_, keys)| keys)
}

/// Whether `value` appears in `text`, ignoring case.
fn contains_ignore_case(text: &str, value: &str) -> bool {
    dotnet::ordinal_ignore_case_key(text).contains(&dotnet::ordinal_ignore_case_key(value))
}

#[cfg(test)]
#[path = "last_fm_radio_recommendation_service_tests.rs"]
mod tests;
