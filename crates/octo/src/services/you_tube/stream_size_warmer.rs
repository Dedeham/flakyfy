//! Learns, in the background, the true size of the stream an outside song plays, for the rows
//! that went out without one. Searches and plays learn most sizes as they go (the shim names the
//! stream in the same run that picks it); this fills the listed rows nothing else reached, so
//! the next answer that lists them can carry the size.
//!
//! Gentle on YouTube by construction: at most [`PER_MINUTE`] lookups a minute, [`CONCURRENT`]
//! at a time, one queued entry per song, a song tried at most once an hour, and a full queue
//! drops new rows rather than growing.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use octo_core::soulseek::{LengthSource, RoutingKind, SongLength, SoulseekRouting, StreamFacts};
use octo_subsonic::subsonic_response_builder::IdRegistry;
use parking_lot::Mutex;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::you_tube_resolver::{YouTubeHit, YouTubeResolver};
use crate::services::soulseek::ExternalIdRegistry;

/// How many songs can wait at once; past this, new rows are dropped.
pub const QUEUE_CAPACITY: usize = 512;
/// Lookups started per minute, at most.
pub const PER_MINUTE: usize = 30;
/// Lookups running at once, at most.
pub const CONCURRENT: usize = 2;
/// How long a song that was tried is left alone, answered or not.
pub const RETRY_AFTER: Duration = Duration::from_secs(3600);
/// How many tried songs are remembered; the oldest are forgotten first.
const TRIED_MAX: usize = 4096;
/// How long after a play its measured loudness is asked for: the shim measures while the song
/// plays, which takes a few seconds for a whole song.
pub const LOUDNESS_PICKUP_DELAY: Duration = Duration::from_secs(90);

/// Where the warmer asks: the shim, as background work.
#[async_trait]
pub trait StreamFactsSource: Send + Sync {
    /// The stream facts of a video already pinned.
    async fn facts(&self, video_id: &str) -> Option<StreamFacts>;
    /// The video a play would pick for this query and length, with its stream.
    async fn search(&self, query: &str, duration_hint: Option<i32>) -> Option<YouTubeHit>;
}

#[async_trait]
impl StreamFactsSource for YouTubeResolver {
    async fn facts(&self, video_id: &str) -> Option<StreamFacts> {
        YouTubeResolver::facts(self, video_id, true).await
    }

    async fn search(&self, query: &str, duration_hint: Option<i32>) -> Option<YouTubeHit> {
        YouTubeResolver::search(self, query, duration_hint, true).await
    }
}

/// A sliding minute of lookup starts.
#[derive(Debug)]
pub struct RateWindow {
    per_minute: usize,
    starts: VecDeque<Instant>,
}

impl RateWindow {
    pub fn new(per_minute: usize) -> Self {
        RateWindow {
            per_minute: per_minute.max(1),
            starts: VecDeque::new(),
        }
    }

    /// None when a lookup may start at `now` (and counts it); else how long to wait first.
    pub fn wait_before_next(&mut self, now: Instant) -> Option<Duration> {
        const MINUTE: Duration = Duration::from_secs(60);
        while self
            .starts
            .front()
            .is_some_and(|start| now.duration_since(*start) >= MINUTE)
        {
            self.starts.pop_front();
        }
        if self.starts.len() < self.per_minute {
            self.starts.push_back(now);
            return None;
        }
        let oldest = *self.starts.front()?;
        Some(MINUTE.saturating_sub(now.duration_since(oldest)))
    }
}

/// The background sizer, and the id registry the response builder reads stream facts through
/// (so every row that goes out without a size asks it to learn one).
pub struct StreamSizeWarmer {
    registry: Arc<ExternalIdRegistry>,
    source: Arc<dyn StreamFactsSource>,
    sender: mpsc::Sender<String>,
    receiver: Mutex<Option<mpsc::Receiver<String>>>,
    queued: Mutex<HashSet<String>>,
    tried: Mutex<HashMap<String, Instant>>,
    /// Songs played since their loudness was last asked for.
    loudness_wanted: Mutex<HashSet<String>>,
}

impl StreamSizeWarmer {
    pub fn new(registry: Arc<ExternalIdRegistry>, source: Arc<dyn StreamFactsSource>) -> Self {
        let (sender, receiver) = mpsc::channel(QUEUE_CAPACITY);
        StreamSizeWarmer {
            registry,
            source,
            sender,
            receiver: Mutex::new(Some(receiver)),
            queued: Mutex::new(HashSet::new()),
            tried: Mutex::new(HashMap::new()),
            loudness_wanted: Mutex::new(HashSet::new()),
        }
    }

    /// Queues `song_id` when it is an outside song with no known size that was not tried in
    /// the last hour and is not already waiting. True when it was queued.
    pub fn want(&self, song_id: &str) -> bool {
        if song_id.is_empty() || !self.needs_size(song_id) {
            return false;
        }
        if self
            .tried
            .lock()
            .get(song_id)
            .is_some_and(|at| at.elapsed() < RETRY_AFTER)
        {
            return false;
        }
        if !self.queued.lock().insert(song_id.to_string()) {
            return false;
        }
        if self.sender.try_send(song_id.to_string()).is_err() {
            // Full: dropped, and free to be listed again later.
            self.queued.lock().remove(song_id);
            return false;
        }
        true
    }

    fn needs_size(&self, song_id: &str) -> bool {
        self.registry.lookup(song_id).is_some_and(|routing| {
            let r = routing.lock();
            r.kind == RoutingKind::Song
                && r.stream_size.is_none()
                && (r.has_you_tube() || r.has_artist_title())
        })
    }

    /// Learns the size of one song: the pinned video's facts, or, with no video pinned yet, the
    /// same search a play would make, pinned so the size is the size of what plays.
    pub async fn warm_one(&self, song_id: &str) {
        let Some(routing) = self.registry.lookup(song_id) else {
            return;
        };
        let r: SoulseekRouting = routing.snapshot();
        let loudness = self.loudness_wanted.lock().remove(song_id) && r.track_gain.is_none();
        if r.kind != RoutingKind::Song || (r.stream_size.is_some() && !loudness) {
            return;
        }
        if let Some(video) = r.you_tube_id.clone().filter(|v| !v.is_empty()) {
            if let Some(facts) = self.source.facts(&video).await {
                self.registry.remember_stream(song_id, &video, &facts);
            }
            return;
        }
        if !r.has_artist_title() {
            return;
        }
        let query = format!(
            "{} {}",
            r.artist.as_deref().unwrap_or(""),
            r.title.as_deref().unwrap_or("")
        );
        let Some(hit) = self
            .source
            .search(&query, r.duration)
            .await
            .filter(|h| !h.video_id.is_empty())
        else {
            return;
        };
        // A play that pinned a video meanwhile keeps it.
        if !self.registry.pin_video(song_id, &hit.video_id, false) {
            return;
        }
        self.registry.remember_stream(song_id, &hit.video_id, &hit.stream);
        if let (Some(d), Some(routing)) = (hit.duration, self.registry.lookup(song_id)) {
            let mut r = routing.lock();
            if SongLength::stream_length_may_replace(r.duration, d) {
                r.duration = Some(d);
            }
        }
        self.registry
            .remember_length(song_id, hit.duration, LengthSource::Video);
    }

    /// A play of `song_id` started: the shim measures its loudness while it plays, so ask for
    /// it a little later, unless it is already known.
    pub fn after_play(self: &Arc<Self>, song_id: &str) {
        let known = self
            .registry
            .stream_facts(song_id)
            .is_some_and(|facts| facts.track_gain.is_some());
        if known || song_id.is_empty() {
            return;
        }
        let warmer = self.clone();
        let song_id = song_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(LOUDNESS_PICKUP_DELAY).await;
            warmer.want_loudness(&song_id);
        });
    }

    /// Queues a played song for its measured loudness: one /facts call for its pinned video,
    /// within the same limits as the size lookups. True when it was queued.
    pub fn want_loudness(&self, song_id: &str) -> bool {
        let pinned = self.registry.lookup(song_id).is_some_and(|routing| {
            let r = routing.lock();
            r.kind == RoutingKind::Song && r.has_you_tube() && r.track_gain.is_none()
        });
        if !pinned {
            return false;
        }
        self.loudness_wanted.lock().insert(song_id.to_string());
        if !self.queued.lock().insert(song_id.to_string()) {
            // Already waiting: that lookup brings the loudness too.
            return true;
        }
        if self.sender.try_send(song_id.to_string()).is_err() {
            self.queued.lock().remove(song_id);
            self.loudness_wanted.lock().remove(song_id);
            return false;
        }
        true
    }

    fn done(&self, song_id: &str) {
        self.queued.lock().remove(song_id);
        let mut tried = self.tried.lock();
        tried.insert(song_id.to_string(), Instant::now());
        if tried.len() > TRIED_MAX {
            tried.retain(|_, at| at.elapsed() < RETRY_AFTER);
        }
    }

    /// The worker: takes queued songs at most [`PER_MINUTE`] a minute, [`CONCURRENT`] at a time,
    /// until shutdown.
    pub async fn run(self: Arc<Self>, token: CancellationToken) -> anyhow::Result<()> {
        let Some(mut receiver) = self.receiver.lock().take() else {
            return Ok(());
        };
        let mut window = RateWindow::new(PER_MINUTE);
        let running = Arc::new(Semaphore::new(CONCURRENT));
        loop {
            let song_id = tokio::select! {
                () = token.cancelled() => return Ok(()),
                next = receiver.recv() => match next {
                    Some(song_id) => song_id,
                    None => return Ok(()),
                },
            };
            while let Some(wait) = window.wait_before_next(Instant::now()) {
                tokio::select! {
                    () = token.cancelled() => return Ok(()),
                    () = tokio::time::sleep(wait) => {}
                }
            }
            let permit = tokio::select! {
                () = token.cancelled() => return Ok(()),
                permit = running.clone().acquire_owned() => permit?,
            };
            let warmer = self.clone();
            tokio::spawn(async move {
                warmer.warm_one(&song_id).await;
                warmer.done(&song_id);
                debug!("stream size warm done for {song_id}");
                drop(permit);
            });
        }
    }
}

impl IdRegistry for StreamSizeWarmer {
    fn register(&self, routing: SoulseekRouting) -> String {
        self.registry.register(routing)
    }

    fn stream_facts(&self, song_id: &str) -> Option<StreamFacts> {
        self.registry.stream_facts(song_id)
    }

    fn size_wanted(&self, song_id: &str) {
        self.want(song_id);
    }
}

#[cfg(test)]
#[path = "stream_size_warmer_tests.rs"]
mod tests;
