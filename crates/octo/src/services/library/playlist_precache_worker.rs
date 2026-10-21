//! Fetches a few songs of the lists the admin picked ahead of time, so a shuffle starts at once
//! instead of waiting for a search, and lets them go again when nobody took them up.
//!
//! One worker does both, a minute apart at most: a round of fetching every
//! `PrecacheIntervalHours`, and a look at what was fetched once a day. What it needs from the rest
//! of Octo is behind [`PrecacheWorld`], so the rounds are tested without Navidrome or Soulseek.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::TimeDelta;
use futures::FutureExt;
use octo_core::common::Clock;
use octo_core::library::precache_pick::{self, Candidate, Expiry, Uptake};
use octo_core::settings::{GeneratedPlaylistSettings, SettingsStore};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::precache_store::{Cached, LET_GO_MEMORY_DAYS, LetGo, PrecacheStore};

/// A list that can be picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListInfo {
    /// The listener, in lower case.
    pub owner: String,
    pub key: String,
    pub label: String,
    /// "Made for you" or "Chart".
    pub kind: &'static str,
    /// How many of its songs are from outside the library.
    pub outside: usize,
}

/// What the pre-cache asks of the rest of Octo.
#[async_trait]
pub trait PrecacheWorld: Send + Sync {
    /// Every list that can be picked.
    fn lists(&self) -> Vec<ListInfo>;

    /// A list's songs from outside the library, as last made. Never makes the list.
    fn outside_songs(&self, owner: &str, key: &str) -> Vec<Candidate>;

    /// Someone else's download is queued or running.
    fn busy(&self) -> bool;

    /// The song is in the library already, or queued or downloading for someone.
    async fn already_here(&self, song: &Candidate) -> bool;

    /// Fetches the song and gives the path it landed at.
    async fn fetch(&self, song: &Candidate) -> Result<String, String>;

    fn file_exists(&self, path: &str) -> bool;

    /// Navidrome's id of the fetched song, once it shows it.
    async fn song_id(&self, cached: &Cached) -> Option<String>;

    /// What listeners did with the song.
    async fn uptake(&self, song_id: &str) -> Uptake;

    /// Moves the song to the trash. Err says why it could not.
    async fn let_go(&self, song_id: &str) -> Result<(), String>;
}

pub struct PlaylistPrecacheWorker {
    store: Arc<PrecacheStore>,
    settings: Arc<SettingsStore>,
    world: Arc<dyn PrecacheWorld>,
    clock: Clock,
}

impl PlaylistPrecacheWorker {
    /// How often the worker looks at whether a round is due.
    const TICK: Duration = Duration::from_secs(60);
    /// Let the library settle after a start before downloading anything.
    const START_DELAY: Duration = Duration::from_secs(120);
    /// Fetches that fail in a row before a round gives up until the next tick.
    const MAX_FAILURES: u32 = 3;

    pub fn new(
        store: Arc<PrecacheStore>,
        settings: Arc<SettingsStore>,
        world: Arc<dyn PrecacheWorld>,
        clock: Clock,
    ) -> Self {
        PlaylistPrecacheWorker {
            store,
            settings,
            world,
            clock,
        }
    }

    pub fn store(&self) -> &Arc<PrecacheStore> {
        &self.store
    }

    pub fn world(&self) -> &Arc<dyn PrecacheWorld> {
        &self.world
    }

    pub async fn run(self: Arc<Self>, stopping: CancellationToken) -> anyhow::Result<()> {
        let mut wait = Self::START_DELAY;
        loop {
            tokio::select! {
                () = stopping.cancelled() => return Ok(()),
                () = tokio::time::sleep(wait) => {}
            }
            // One bad round never ends the worker.
            if AssertUnwindSafe(self.tick()).catch_unwind().await.is_err() {
                error!("the pre-cache stopped in the middle of a round; it tries again");
            }
            wait = Self::TICK;
        }
    }

    /// Does what is due: a round of fetching, a look at what was fetched.
    pub async fn tick(&self) {
        let settings = self.settings.current().generated_playlists.clone();
        if !settings.precache {
            return;
        }
        let now = self.clock.now();
        let interval = TimeDelta::hours(i64::from(settings.effective_precache_interval_hours()));
        let fill_due = self
            .store
            .read(|s| s.last_fill_utc.is_none_or(|last| now - last >= interval));
        // Stamped only when the round got through its lists: one cut short by someone's download
        // is tried again at the next tick.
        if fill_due && self.fill(&settings).await {
            self.store.update(|s| s.last_fill_utc = Some(now));
        }
        let expiry_due = self.store.read(|s| {
            s.last_expiry_utc
                .is_none_or(|last| now - last >= TimeDelta::days(1))
        });
        if expiry_due {
            self.expire(&settings).await;
            self.store.update(|s| s.last_expiry_utc = Some(now));
        }
    }

    /// One round: each picked list is brought up to its number of ready songs, a random few of
    /// the ones not here yet, one at a time. False when the round stopped early.
    pub async fn fill(&self, settings: &GeneratedPlaylistSettings) -> bool {
        let picks = self.store.read(|s| s.picks.clone());
        let mut failures = 0;
        for pick in picks {
            let want =
                pick.count
                    .map_or(settings.effective_precache_count(), |n| n.clamp(1, 50)) as usize;
            let have = self.store.read(|s| {
                s.cached
                    .iter()
                    .filter(|c| c.owner == pick.owner && c.list == pick.key)
                    .count()
            });
            if have >= want {
                continue;
            }
            let now = self.clock.now();
            let mut fresh = Vec::new();
            for song in self.world.outside_songs(&pick.owner, &pick.key) {
                let key = song.key();
                let known = self
                    .store
                    .read(|s| s.recently_let_go(&key, now) || s.cached.iter().any(|c| c.key() == key));
                if !known && !self.world.already_here(&song).await {
                    fresh.push(song);
                }
            }
            for song in precache_pick::pick(fresh, want - have, |_| false, |n| rand::random_range(0..n)) {
                if self.world.busy() {
                    return false;
                }
                match self.world.fetch(&song).await {
                    Ok(path) => {
                        info!("pre-cache: {} - {} is ready", song.artist, song.title);
                        let cached = Cached {
                            owner: pick.owner.clone(),
                            list: pick.key.clone(),
                            provider: song.provider.clone(),
                            external_id: song.external_id.clone(),
                            artist: song.artist.clone(),
                            title: song.title.clone(),
                            path,
                            cached_utc: self.clock.now(),
                            note: None,
                        };
                        self.store.update(|s| s.cached.push(cached));
                        failures = 0;
                    }
                    Err(e) => {
                        warn!(
                            "pre-cache: {} - {} could not be fetched: {e}",
                            song.artist, song.title
                        );
                        failures += 1;
                        if failures >= Self::MAX_FAILURES {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    /// Looks at each fetched song old enough: one someone took up becomes an ordinary library
    /// song, one nobody did goes to the trash (or, on a dry run, is only said to). A song whose
    /// state cannot be read is left for the next look.
    pub async fn expire(&self, settings: &GeneratedPlaylistSettings) {
        let keep_days = settings.effective_precache_keep_days();
        let min_plays = settings.effective_precache_min_plays();
        let now = self.clock.now();
        let old = TimeDelta::days(i64::from(keep_days));
        for entry in self.store.read(|s| s.cached.clone()) {
            if now - entry.cached_utc < old {
                continue;
            }
            let key = entry.key();
            if !self.world.file_exists(&entry.path) {
                // Removed by someone else: nothing is left to look after.
                self.store.update(|s| s.cached.retain(|c| c.key() != key));
                continue;
            }
            let Some(song_id) = self.world.song_id(&entry).await else {
                continue;
            };
            let uptake = self.world.uptake(&song_id).await;
            match precache_pick::expiry(entry.cached_utc, now, keep_days, min_plays, uptake) {
                Expiry::NotYet => {}
                Expiry::Unknown(why) => info!(
                    "pre-cache: {} - {} stays for now: {why}",
                    entry.artist, entry.title
                ),
                Expiry::Keep(why) => {
                    info!(
                        "pre-cache: {} - {} is a library song now: {why}",
                        entry.artist, entry.title
                    );
                    self.store.update(|s| s.cached.retain(|c| c.key() != key));
                }
                Expiry::Let if settings.precache_dry_run => {
                    info!(
                        "pre-cache: {} - {} would be let go (dry run)",
                        entry.artist, entry.title
                    );
                    self.note(&key, "dry run: nobody took it up, so it would go to the trash");
                }
                Expiry::Let => match self.world.let_go(&song_id).await {
                    Ok(()) => {
                        info!("pre-cache: {} - {} went to the trash", entry.artist, entry.title);
                        self.store.update(|s| {
                            s.cached.retain(|c| c.key() != key);
                            s.let_go
                                .retain(|gone| now - gone.at_utc < TimeDelta::days(LET_GO_MEMORY_DAYS));
                            s.let_go.push(LetGo {
                                key: key.clone(),
                                at_utc: now,
                            });
                        });
                    }
                    Err(e) => {
                        warn!(
                            "pre-cache: {} - {} could not be let go: {e}",
                            entry.artist, entry.title
                        );
                        self.note(&key, &format!("could not be let go: {e}"));
                    }
                },
            }
        }
    }

    fn note(&self, key: &str, note: &str) {
        self.store.update(|s| {
            if let Some(entry) = s.cached.iter_mut().find(|c| c.key() == key) {
                entry.note = Some(note.to_string());
            }
        });
    }
}

#[cfg(test)]
#[path = "playlist_precache_worker_tests.rs"]
mod tests;
