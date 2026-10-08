//! Port of `Services/Radio/RadioSourceSet.cs`: every radio source, asked together. One slow or
//! failing source costs its own answer, never the radio.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use indexmap::IndexMap;
use octo_core::json::datetime;
use octo_core::radio::radio_blend::{RadioWeights, SOUNDS_ALIKE_WHEN_UNKNOWN};
use octo_core::radio::radio_provider::{LAST_FM, LIBRARY, LISTEN_BRAINZ, SOUNDS_ALIKE, YOU_TUBE_MUSIC};
use octo_core::radio::{RadioAnswer, RadioSeed};
use octo_core::settings::{RadioSourceSettings, SettingsStore};
use parking_lot::Mutex;
use tracing::{info, warn};

use super::radio_outcome_store::RadioOutcomeStore;
use super::radio_source::IRadioSource;

/// Failures or timeouts in a row before a source is rested, and for how long. A source that
/// hangs would otherwise cost every radio its full timeout, and a station build one timeout per
/// seed.
pub const FAILURES_BEFORE_REST: i32 = 3;
pub const LOWEST_WEIGHT: f64 = 0.3;
pub const HIGHEST_WEIGHT: f64 = 1.5;

pub struct RadioSourceSet {
    sources: Vec<Arc<dyn IRadioSource>>,
    /// `IOptionsMonitor<RadioSourceSettings>`.
    settings: Arc<SettingsStore>,
    outcomes: Option<Arc<RadioOutcomeStore>>,
    source_timeout: Mutex<Duration>,
    rest: Mutex<TimeDelta>,
    /// Failures in a row, and until when a source rests.
    health: Mutex<HashMap<String, (i32, DateTime<Utc>)>>,
}

impl RadioSourceSet {
    pub fn new(
        sources: Vec<Arc<dyn IRadioSource>>,
        settings: Arc<SettingsStore>,
        outcomes: Option<Arc<RadioOutcomeStore>>,
    ) -> Self {
        RadioSourceSet {
            sources,
            settings,
            outcomes,
            source_timeout: Mutex::new(Duration::from_secs(8)),
            rest: Mutex::new(TimeDelta::minutes(5)),
            health: Mutex::new(HashMap::new()),
        }
    }

    /// `SourceTimeout = ...`, for tests.
    pub fn set_source_timeout(&self, timeout: Duration) {
        *self.source_timeout.lock() = timeout;
    }

    /// `Rest = ...`, for tests.
    pub fn set_rest(&self, rest: TimeDelta) {
        *self.rest.lock() = rest;
    }

    pub fn available(&self) -> Vec<Arc<dyn IRadioSource>> {
        let now = Utc::now();
        let health = self.health.lock();
        self.sources
            .iter()
            .filter(|source| {
                source.available()
                    && !health
                        .get(source.provider())
                        .is_some_and(|(_, rest_until)| *rest_until > now)
            })
            .cloned()
            .collect()
    }

    /// What each source counts for this radio: its base weight, raised for Sounds alike when no
    /// catalog knows the song, times what this listener's plays have taught (1 with no data),
    /// kept within [`LOWEST_WEIGHT`] and [`HIGHEST_WEIGHT`]. A base of 0 means the admin left the
    /// source out, and stays 0.
    pub fn weights(&self, listener: Option<&str>, library_led: bool) -> RadioWeights {
        let current = self.settings.current().radio_sources.clone();
        let learned = self
            .outcomes
            .as_ref()
            .map(|outcomes| outcomes.multipliers(listener, None))
            .unwrap_or_default();
        let mut sound = RadioSourceSettings::effective_weight(current.sounds_alike_weight);
        if library_led && sound > 0.0 {
            sound = sound.max(SOUNDS_ALIKE_WHEN_UNKNOWN);
        }
        // The bounds never undo the admin: a base set below 0.3 or above 1.5 is itself the bound.
        let weigh = |provider: &str, base: f64| {
            if base <= 0.0 {
                0.0
            } else {
                (base * learned.get(provider).copied().unwrap_or(1.0))
                    .clamp(LOWEST_WEIGHT.min(base), HIGHEST_WEIGHT.max(base))
            }
        };
        IndexMap::from([
            (
                LAST_FM.to_string(),
                weigh(
                    LAST_FM,
                    RadioSourceSettings::effective_weight(current.last_fm_weight),
                ),
            ),
            (
                YOU_TUBE_MUSIC.to_string(),
                weigh(
                    YOU_TUBE_MUSIC,
                    RadioSourceSettings::effective_weight(current.you_tube_music_weight),
                ),
            ),
            (
                LISTEN_BRAINZ.to_string(),
                weigh(
                    LISTEN_BRAINZ,
                    RadioSourceSettings::effective_weight(current.listen_brainz_weight),
                ),
            ),
            (SOUNDS_ALIKE.to_string(), weigh(SOUNDS_ALIKE, sound)),
            (LIBRARY.to_string(), 1.0),
        ])
    }

    /// `only`: the sources to ask; all available ones when None. An artist station that has
    /// already asked Last.fm itself leaves Last.fm out.
    pub async fn ask_all(
        &self,
        seed: &RadioSeed,
        count: i32,
        auth: &IndexMap<String, String>,
        only: Option<&(dyn Fn(&dyn IRadioSource) -> bool + Sync)>,
    ) -> Vec<RadioAnswer> {
        let asked: Vec<Arc<dyn IRadioSource>> = self
            .available()
            .into_iter()
            .filter(|source| only.is_none_or(|only| only(source.as_ref())))
            .collect();
        futures::future::join_all(
            asked
                .iter()
                .map(|source| self.ask(source.as_ref(), source.songs_like(seed, count, auth))),
        )
        .await
    }

    pub async fn tag_all(&self, tag: &str, count: i32) -> Vec<RadioAnswer> {
        let asked = self.available();
        futures::future::join_all(
            asked
                .iter()
                .map(|source| self.ask(source.as_ref(), source.tag(tag, count))),
        )
        .await
    }

    async fn ask(
        &self,
        source: &dyn IRadioSource,
        ask: impl Future<Output = anyhow::Result<RadioAnswer>>,
    ) -> RadioAnswer {
        let timeout = *self.source_timeout.lock();
        match tokio::time::timeout(timeout, ask).await {
            Ok(Ok(answer)) => {
                // A success clears the count of failures in a row, but never cuts a rest short:
                // an answer to a call made before the rest began says nothing about the source
                // now.
                let mut health = self.health.lock();
                let entry = health
                    .entry(source.provider().to_string())
                    .or_insert((0, datetime::min_value()));
                entry.0 = 0;
                answer
            }
            Err(_) => {
                info!(
                    "Radio source {} took longer than {} s; left out",
                    source.provider(),
                    timeout.as_secs_f64()
                );
                self.failed(source.provider());
                RadioAnswer::nothing(source.provider())
            }
            Ok(Err(error)) => {
                warn!("Radio source {} failed; left out: {error}", source.provider());
                self.failed(source.provider());
                RadioAnswer::nothing(source.provider())
            }
        }
    }

    fn failed(&self, provider: &str) {
        let mut health = self.health.lock();
        let entry = health
            .entry(provider.to_string())
            .or_insert((0, datetime::min_value()));
        entry.0 += 1;
        if entry.0 < FAILURES_BEFORE_REST {
            return;
        }
        let rest = *self.rest.lock();
        *entry = (0, Utc::now() + rest);
        warn!(
            "Radio source {provider} failed {FAILURES_BEFORE_REST} times in a row; resting it for {} minutes",
            rest.num_seconds() as f64 / 60.0
        );
    }
}
