//! Port of `Services/LastFm/LastFmRadioRecommendationService.cs`: builds canonical station
//! snapshots from bounded listening signals, asking Last.fm for the candidates. The weighing,
//! the draw and the shaping are `octo_core::last_fm::last_fm_radio_recommendation_service`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use indexmap::IndexMap;
use octo_core::common::dotnet;
use octo_core::last_fm::SimilarTrack;
use octo_core::last_fm::last_fm_radio_recommendation_service::{
    Candidate, HEARTED_SEED_AFFINITY, IgnoreCaseKeys, NEIGHBOUR_ARTIST_AFFINITY, Randomizer,
    SEED_RECENCY_DECAY, SEEDS_AT_ONCE, artist_cap, artist_key, create, default_randomizer,
    definition_version, key, ranked, score_artists, score_local_tags, seed_score, shape, source_weight,
    suppress_station_overlap, title, to_candidate,
};
use octo_core::last_fm::last_fm_radio_seed_normalizer as seed_normalizer;
use octo_core::models::domain::Song;
use octo_core::models::radio::{LastFmRadioPlay, LastFmRadioStation, LastFmRadioStationKind};
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

/// `CancellationTokenSource.CreateLinkedTokenSource(ct)` with `CancelAfter(25 s)`.
struct Budget<'a> {
    caller: &'a CancellationToken,
    deadline: Instant,
}

impl Budget<'_> {
    /// The call, or [`OperationCanceled`] once the caller cancels or the deadline passes. (A
    /// cancelled call is dropped, which cancels its request.)
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
    /// `IOptionsMonitor<LastFmSettings>`.
    settings: Arc<SettingsStore>,
    /// Source of the draw. Tests replace it with a seeded one so a build repeats exactly.
    randomizer: Mutex<Randomizer>,
    /// Every radio source; None keeps stations on Last.fm alone.
    sources: Option<Arc<RadioSourceSet>>,
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
        }
    }

    /// `Randomizer = ...`.
    pub fn set_randomizer(&self, randomizer: Randomizer) {
        *self.randomizer.lock() = randomizer;
    }

    /// The listener's stations, built now. An error is what the C# threw out of `BuildAsync`: an
    /// answer Last.fm's reader refused, or [`OperationCanceled`] when the caller cancelled or the
    /// budget ran out while an artist station was being built.
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
        // GroupBy(Key, OrdinalIgnoreCase): the first station with a key names its snapshot.
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
        let refill_headroom = (unavailable.len() as i32).min(settings.effective_radio_track_count());
        let candidate_target = 100.min(settings.effective_radio_track_count() + refill_headroom + 10);
        let artist_scores = score_artists(&plays, now);
        // Seeds are the strongest recent signals, not simply the newest plays: a heart outranks
        // a play, a chosen play outranks one the radio served, and age decays.
        let mut by_seed_score: Vec<&LastFmRadioPlay> = plays.iter().collect();
        by_seed_score.sort_by(|a, b| {
            seed_score(b, now)
                .partial_cmp(&seed_score(a, now))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut seen_seeds = std::collections::HashSet::new();
        let track_seeds: Vec<LastFmRadioPlay> = by_seed_score
            .into_iter()
            .filter(|play| seen_seeds.insert(seed_normalizer::track_key(&play.artist, &play.title)))
            .take(8)
            .cloned()
            .collect();
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
                    let count = settings.effective_radio_track_count();
                    Some(
                        count
                            - dotnet::round(
                                f64::from(count) * f64::from(settings.effective_discovery_percent()) / 100.0,
                                0,
                            ) as i32,
                    )
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
                    if learned { "Your Mix" } else { "Starter Radio" },
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
                if settings.enable_discovery_mix && !top_tags.is_empty() {
                    let discovery = self
                        .tracks_from_tags(username, &top_tags, candidate_target, &budget)
                        .await?;
                    if discovery.len() >= 5 {
                        let tracks = shape(
                            discovery,
                            &plays,
                            settings,
                            &unavailable,
                            &mut *random,
                            previous("discovery"),
                            artist_cap(settings, LastFmRadioStationKind::Discovery),
                            true,
                            None,
                            Some(&banned),
                            Some(&disliked),
                        );
                        stations.push(create(
                            username,
                            "discovery",
                            "Discovery Mix",
                            LastFmRadioStationKind::Discovery,
                            true,
                            &top_tags,
                            tracks,
                            1,
                            Utc::now(),
                        ));
                    }
                }

                for (artist, _) in artist_scores
                    .iter()
                    .take(settings.effective_artist_station_count().max(0) as usize)
                {
                    let candidates = self
                        .tracks_from_artist(username, artist, candidate_target, &budget)
                        .await?;
                    let station_key = format!("artist-{}", key(artist));
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

                for tag in tags.top_by_score(settings.effective_genre_station_count().max(0) as usize) {
                    let candidates = self
                        .tracks_from_tags(username, std::slice::from_ref(&tag), candidate_target, &budget)
                        .await?;
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
    /// [`NEIGHBOUR_ARTIST_AFFINITY`] so the station stays about who it is named for.
    async fn tracks_from_artist(
        &self,
        username: &str,
        artist: &str,
        candidate_target: i32,
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
        let Ok(similar) = budget.run(self.last_fm.get_similar_artists(artist, 6)).await else {
            return Ok(result);
        };
        for similar in similar?.into_iter().take(5) {
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
}

/// `previousSnapshots.GetValueOrDefault(stationKey) ?? new HashSet<string>()`.
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

/// `string.Contains(value, StringComparison.OrdinalIgnoreCase)`.
fn contains_ignore_case(text: &str, value: &str) -> bool {
    dotnet::ordinal_ignore_case_key(text).contains(&dotnet::ordinal_ignore_case_key(value))
}

#[cfg(test)]
#[path = "last_fm_radio_recommendation_service_tests.rs"]
mod tests;
