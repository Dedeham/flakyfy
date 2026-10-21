//! The one-time YouTube check of the library (`<config>/youtube-check.json`).
//!
//! Every song already in the library is compared with its official YouTube Music copy, once, after
//! a person starts it from the dashboard. A song that sounds nothing like its copy becomes a Review
//! question; nothing is ever changed or removed. It goes a few songs an hour, only while nothing
//! is downloading, only of whoever keeps the library, and carries on where it stopped after a
//! restart: the files are walked once in order and the cursor is the only memory a run needs. A
//! YouTube Music or shim that could not answer is not an answer and is tried again, but not for
//! ever: a song that cannot be answered after two rests is counted as having no copy.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::FutureExt;
use octo_core::common::Clock;
use octo_core::fingerprint::fingerprint_compare::CrossCheck;
use octo_core::fingerprint::verification::{InconclusiveReason, VerificationResult, VerificationVerdict};
use octo_core::json::datetime;
use octo_core::models::domain::Song;
use octo_core::settings::SettingsStore;
use octo_media::audio::AudioFingerprinter;
use octo_media::tags::TagFile;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::library_review_sweep_worker::LibraryReviewSweepWorker;
use super::navidrome_song_path_resolver::get_full_path;
use super::notice_queue::{NoticeOrigin, NoticeQueue};
use super::quality_upgrade_worker::compare_ordinal;
use crate::services::common::IAcquisitionActivity;
use crate::services::family::{LibraryScope, ScopeResolver, ScopeView};
use crate::services::fingerprint::you_tube_cross_check::{Compared, compare};
use crate::services::library::library_walk;
use crate::services::state_file;
use crate::services::you_tube::YouTubeResolver;

/// What the check asks of YouTube, behind a seam so tests need no shim and no fpcalc.
#[async_trait]
pub trait YoutubeChecker: Send + Sync {
    /// The file of `song` against the song's YouTube Music copy.
    async fn check(&self, path: &Path, song: &Song) -> Compared;
}

/// [`compare`] with the app's fingerprinter, YouTube and settings.
pub struct AppYoutubeChecker {
    pub fingerprinter: Arc<AudioFingerprinter>,
    pub youtube: Arc<YouTubeResolver>,
    pub settings: Arc<SettingsStore>,
}

#[async_trait]
impl YoutubeChecker for AppYoutubeChecker {
    async fn check(&self, path: &Path, song: &Song) -> Compared {
        // The length narrows YouTube Music's rows to the song's own; a file that does not say how
        // long it is is looked up by its names alone.
        let length = TagFile::open(path)
            .ok()
            .map(|file| file.duration_seconds())
            .filter(|seconds| *seconds > 0);
        let settings = self.settings.current().soulseek.clone();
        compare(
            &self.fingerprinter,
            &self.youtube,
            path,
            &song.artist,
            &song.title,
            length,
            &settings,
        )
        .await
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct YoutubeCheckState {
    /// A run is going: started from the dashboard and not yet finished.
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub paused: bool,
    /// Library-relative path of the last file dealt with this run; empty at its start.
    #[serde(default, deserialize_with = "state_file::null_as_default")]
    pub cursor: String,
    #[serde(default)]
    pub total: i32,
    /// Songs looked at this run.
    #[serde(default)]
    pub done: i32,
    /// Questions raised.
    #[serde(default)]
    pub asked: i32,
    /// Songs that sound like their copy.
    #[serde(default)]
    pub alike: i32,
    /// Songs alike enough to doubt: another version of the song, left alone.
    #[serde(default)]
    pub unsure: i32,
    /// Songs with no copy to compare with, or no names to look one up by.
    #[serde(default)]
    pub no_copy: i32,
    #[serde(default, with = "datetime::utc_option")]
    pub started_utc: Option<DateTime<Utc>>,
    #[serde(default, with = "datetime::utc_option")]
    pub finished_utc: Option<DateTime<Utc>>,
}

struct StoreInner {
    state: YoutubeCheckState,
    dirty: bool,
}

/// youtube-check.json, written by [`YoutubeCheckStore::run_flusher`] every few seconds, not on
/// every change, and at once when a run starts, pauses or finishes.
pub struct YoutubeCheckStore {
    path: Option<PathBuf>,
    inner: Mutex<StoreInner>,
    flush_lock: Mutex<()>,
}

impl YoutubeCheckStore {
    pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

    /// A store kept in `path` (None keeps it in memory). A file that cannot be read is set aside
    /// as `<file>.corrupt-<ticks>` and the check starts over.
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut state = YoutubeCheckState::default();
        if let Some(path) = &path
            && path.exists()
        {
            match Self::load(path) {
                Ok(loaded) => state = loaded,
                Err(e) => {
                    warn!("YouTube check state could not be read ({e}); starting over");
                    super::notice_queue::set_aside(path);
                }
            }
        }
        YoutubeCheckStore {
            path,
            inner: Mutex::new(StoreInner { state, dirty: false }),
            flush_lock: Mutex::new(()),
        }
    }

    fn load(path: &Path) -> anyhow::Result<YoutubeCheckState> {
        let text = state_file::read_all_text(path)?;
        Ok(serde_json::from_str::<Option<YoutubeCheckState>>(&text)?.unwrap_or_default())
    }

    pub fn read<T>(&self, read: impl FnOnce(&YoutubeCheckState) -> T) -> T {
        read(&self.inner.lock().state)
    }

    /// Changes the state in memory; it reaches the disk on the next timer tick or [`Self::flush`].
    pub fn update(&self, change: impl FnOnce(&mut YoutubeCheckState)) {
        let mut inner = self.inner.lock();
        change(&mut inner.state);
        inner.dirty = true;
    }

    /// Writes the state if it changed. The snapshot is taken inside the same lock as the write,
    /// so an older snapshot can never land after a newer one.
    pub fn flush(&self) -> bool {
        let Some(path) = self.path.as_deref() else {
            return true;
        };
        let _flushing = self.flush_lock.lock();
        let json = {
            let mut inner = self.inner.lock();
            if !inner.dirty {
                return true;
            }
            inner.dirty = false;
            octo_core::json::to_string(&inner.state)
        };
        match state_file::save_atomic(path, &json) {
            Ok(()) => true,
            Err(e) => {
                self.inner.lock().dirty = true;
                warn!("YouTube check state could not be written: {e}");
                false
            }
        }
    }

    /// The flush timer, and the last flush on shutdown, as a worker.
    pub async fn run_flusher(self: Arc<Self>, token: CancellationToken) -> anyhow::Result<()> {
        let store = self.clone();
        state_file::flush_every(
            Self::FLUSH_INTERVAL,
            token,
            Arc::new(move || {
                store.flush();
            }),
        )
        .await
    }
}

impl Drop for YoutubeCheckStore {
    fn drop(&mut self) {
        self.flush();
    }
}

/// What the dashboard shows of the check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YoutubeCheckStatus {
    pub state: String,
    pub reason: Option<String>,
    pub running: bool,
    pub paused: bool,
    pub position: usize,
    pub total: i32,
    pub done: i32,
    pub asked: i32,
    pub alike: i32,
    pub unsure: i32,
    pub no_copy: i32,
    pub open: usize,
    pub keeper: Option<String>,
    pub per_hour: i32,
    pub started_utc: Option<DateTime<Utc>>,
    pub finished_utc: Option<DateTime<Utc>>,
}

/// Compares every library song with its YouTube Music copy, once.
pub struct LibraryYoutubeCheck {
    store: Arc<YoutubeCheckStore>,
    notices: Arc<NoticeQueue>,
    checker: Arc<dyn YoutubeChecker>,
    activity: Arc<dyn IAcquisitionActivity>,
    settings: Arc<SettingsStore>,
    music_root: Box<dyn Fn() -> String + Send + Sync>,
    clock: Clock,
    /// With Family on, the check keeps to the shared library: a member's own songs are never
    /// asked about.
    scope: Option<ScopeResolver>,

    files: Mutex<Option<Arc<Vec<String>>>>,
    /// Changes when a run starts, so work in flight for the last one drops its result.
    generation: AtomicI32,
    unavailable_in_a_row: AtomicI32,
    /// The song YouTube could not be asked about, and how many rests it has cost.
    stuck: Mutex<(String, i32)>,
    hold: Mutex<(String, Option<String>)>,
}

impl LibraryYoutubeCheck {
    pub const MAX_OPEN_QUESTIONS: usize = 50;
    /// Answers in a row YouTube Music or the shim could not give before the check rests.
    pub const MAX_UNAVAILABLE_IN_A_ROW: i32 = 5;
    pub const REST_WHEN_UNAVAILABLE: Duration = Duration::from_secs(3600);
    /// Rests one song may cost before it is counted as having no copy, so a video that never plays
    /// (region-locked, age-gated) cannot hold the whole run.
    pub const MAX_RESTS_PER_SONG: i32 = 2;
    pub const IDLE_CHECK: Duration = Duration::from_secs(60);
    pub const BUSY_CHECK: Duration = Duration::from_secs(30);

    pub fn new(
        store: Arc<YoutubeCheckStore>,
        notices: Arc<NoticeQueue>,
        checker: Arc<dyn YoutubeChecker>,
        activity: Arc<dyn IAcquisitionActivity>,
        settings: Arc<SettingsStore>,
        music_root: Box<dyn Fn() -> String + Send + Sync>,
        clock: Clock,
    ) -> Self {
        LibraryYoutubeCheck {
            store,
            notices,
            checker,
            activity,
            settings,
            music_root,
            clock,
            scope: None,
            files: Mutex::new(None),
            generation: AtomicI32::new(0),
            unavailable_in_a_row: AtomicI32::new(0),
            stuck: Mutex::new((String::new(), 0)),
            hold: Mutex::new(("Idle".to_string(), None)),
        }
    }

    pub fn with_library_scope(mut self, scope: ScopeResolver) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn store(&self) -> &Arc<YoutubeCheckStore> {
        &self.store
    }

    fn shared_view(&self) -> Option<ScopeView> {
        self.scope
            .as_ref()
            .filter(|scope| scope.keeps_shared())
            .map(ScopeResolver::shared_view)
    }

    fn in_shared(view: Option<&ScopeView>, full: &str) -> bool {
        view.is_none_or(|view| view.allows(&LibraryScope::Shared, None, Some(full)))
    }

    fn hold(&self, state: &str, reason: Option<&str>, wait: Duration) -> Duration {
        *self.hold.lock() = (state.to_string(), reason.map(str::to_string));
        wait
    }

    /// Starts a run over the whole library: every earlier answer is forgotten.
    pub fn start(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.files.lock() = None;
        self.unavailable_in_a_row.store(0, Ordering::SeqCst);
        *self.stuck.lock() = (String::new(), 0);
        // Said at once: the worker may be resting, and the page must not say "not started".
        self.hold(
            "Waiting",
            Some("Starting: the first song is compared within a minute or two."),
            Duration::ZERO,
        );
        let now = self.clock.now();
        self.store.update(|s| {
            *s = YoutubeCheckState {
                running: true,
                started_utc: Some(now),
                ..Default::default()
            };
        });
        self.store.flush();
    }

    pub fn set_paused(&self, paused: bool) {
        self.store.update(|s| s.paused = paused);
        self.store.flush();
    }

    /// The worker loop until the host stops, then a last flush.
    pub async fn run(self: Arc<Self>, stopping: CancellationToken) -> anyhow::Result<()> {
        // Never alongside startup, which already works the disk and Navidrome.
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(120)) => {}
            () = stopping.cancelled() => {
                self.store.flush();
                return Ok(());
            }
        }
        loop {
            // A tick that panics is logged and the loop goes on.
            let wait = match std::panic::AssertUnwindSafe(self.tick()).catch_unwind().await {
                Ok(wait) => wait,
                Err(_) => {
                    error!("the YouTube check stopped in the middle of a song; it tries again");
                    Self::IDLE_CHECK
                }
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = stopping.cancelled() => {
                    self.store.flush();
                    return Ok(());
                }
            }
        }
    }

    /// One look: waits, finishes, or checks one song. How long to rest before the next.
    pub async fn tick(&self) -> Duration {
        if !self.store.read(|s| s.running) {
            return self.hold(
                "Idle",
                Some("Press the button to compare every song with its YouTube copy once."),
                Self::IDLE_CHECK,
            );
        }
        let current = self.settings.current();
        let actions = current.library_actions.clone();
        let per_hour = actions.effective_youtube_check_per_hour();
        if !actions.enabled || !actions.review_enabled || per_hour == 0 {
            return self.hold(
                "Off",
                Some("Set how many songs an hour to check, with library actions and Review on."),
                Self::IDLE_CHECK,
            );
        }
        if self.store.read(|s| s.paused) {
            return self.hold("Paused", Some("Paused from the dashboard."), Self::IDLE_CHECK);
        }
        let admin = current.subsonic.admin_username.clone();
        let Some(keeper) = LibraryReviewSweepWorker::keeper(&actions, admin.as_deref()) else {
            return self.hold(
                "Paused",
                Some("Nobody is on the library actions allowlist to ask."),
                Self::IDLE_CHECK,
            );
        };
        let open = self.notices.open_count(NoticeOrigin::LibrarySweep);
        if open >= Self::MAX_OPEN_QUESTIONS {
            let reason =
                format!("{open} library questions are waiting in Review. Answer some and it carries on.");
            return self.hold("Paused", Some(&reason), Self::IDLE_CHECK);
        }
        if self.activity.is_busy() {
            return self.hold(
                "Waiting",
                Some("Waiting for a download to finish."),
                Self::BUSY_CHECK,
            );
        }

        let generation = self.generation.load(Ordering::SeqCst);
        let root = (self.music_root)();
        if self.files.lock().is_none() {
            if !Path::new(&root).is_dir() {
                return self.hold(
                    "Paused",
                    Some("The music folder cannot be read."),
                    Self::IDLE_CHECK,
                );
            }
            let trash = library_walk::trash_folder(Path::new(&root), &actions);
            let listed = Arc::new(LibraryReviewSweepWorker::enumerate(&root, Some(&trash)));
            let total = i32::try_from(listed.len()).unwrap_or(i32::MAX);
            // Start was pressed while the library was being listed: list again next tick.
            if generation != self.generation.load(Ordering::SeqCst) {
                return Duration::ZERO;
            }
            *self.files.lock() = Some(listed);
            self.store.update(|s| s.total = total);
        }
        let Some(files) = self.files.lock().clone() else {
            return Duration::ZERO;
        };
        let interval = Duration::from_secs_f64(3600.0 / f64::from(per_hour));
        let cursor = self.store.read(|s| s.cursor.clone());
        let Some(target) = self.next_target(&files, &root, cursor) else {
            // Nothing is left: the run is over, and it does not start again by itself.
            let now = self.clock.now();
            self.store.update(|s| {
                s.running = false;
                s.cursor = String::new();
                s.finished_utc = Some(now);
            });
            self.store.flush();
            *self.files.lock() = None;
            info!("YouTube check of the library finished");
            return self.hold(
                "Done",
                Some("Every song has been compared with its YouTube copy."),
                Self::IDLE_CHECK,
            );
        };
        self.check(generation, &root, &keeper, &target, interval).await
    }

    /// The next file after `cursor` to check. A file that is gone, outside the shared library or
    /// already asked about is passed over.
    fn next_target(&self, files: &[String], root: &str, cursor: String) -> Option<String> {
        let view = self.shared_view();
        let reviewed = self.notices.reviewed_paths();
        files[first_after(files, &cursor)..].iter().find_map(|relative| {
            let full = full_path(root, relative);
            let there = std::fs::metadata(&full).is_ok_and(|m| m.is_file());
            (there && Self::in_shared(view.as_ref(), &full) && !reviewed.contains(&full))
                .then(|| relative.clone())
        })
    }

    async fn check(
        &self,
        generation: i32,
        root: &str,
        keeper: &str,
        target: &str,
        interval: Duration,
    ) -> Duration {
        let full = full_path(root, target);
        let swept = LibraryReviewSweepWorker::read_tags(&full);
        let song = swept.song;
        // A song with no artist and title tag has no names to look its copy up by.
        let answer = if swept.tagged {
            self.checker.check(Path::new(&full), &song).await
        } else {
            Compared::NoReference
        };
        // Start was pressed meanwhile: this answer belongs to the last run.
        if generation != self.generation.load(Ordering::SeqCst) {
            return Duration::ZERO;
        }
        if answer == Compared::Unavailable {
            let in_a_row = self.unavailable_in_a_row.fetch_add(1, Ordering::SeqCst) + 1;
            if in_a_row < Self::MAX_UNAVAILABLE_IN_A_ROW {
                return self.hold("Running", None, interval);
            }
            self.unavailable_in_a_row.store(0, Ordering::SeqCst);
            let rests = {
                let mut stuck = self.stuck.lock();
                if stuck.0 == target {
                    stuck.1 += 1;
                } else {
                    *stuck = (target.to_string(), 1);
                }
                stuck.1
            };
            if rests < Self::MAX_RESTS_PER_SONG {
                warn!(
                    "YouTube could not be asked {in_a_row} times in a row; the YouTube check rests an hour"
                );
                return self.hold(
                    "Waiting",
                    Some("YouTube Music is not answering. Trying again in an hour."),
                    Self::REST_WHEN_UNAVAILABLE,
                );
            }
            // Two rests on the same song: it is the song, not YouTube. Counted as having no copy.
            warn!("The YouTube check gives up on {target}: it could not be compared after {rests} rests");
            *self.stuck.lock() = (String::new(), 0);
        } else {
            self.unavailable_in_a_row.store(0, Ordering::SeqCst);
        }

        let mut asked = false;
        let (mut alike, mut unsure, mut no_copy) = (0, 0, 0);
        match answer {
            Compared::Done(outcome) => match outcome.verdict {
                CrossCheck::Differs => {
                    let question = VerificationResult {
                        verdict: VerificationVerdict::Inconclusive,
                        reason: InconclusiveReason::DiffersFromYouTube,
                        evidence: Some(format!("{}% alike", (outcome.similarity * 100.0).round())),
                        ..Default::default()
                    };
                    asked = self.notices.add_review_from(
                        keeper,
                        &full,
                        &song,
                        &question,
                        NoticeOrigin::LibrarySweep,
                    );
                }
                CrossCheck::Match => alike = 1,
                CrossCheck::Unsure => unsure = 1,
            },
            _ => no_copy = 1,
        }
        self.store.update(|s| {
            s.cursor = target.to_string();
            s.done += 1;
            s.asked += i32::from(asked);
            s.alike += alike;
            s.unsure += unsure;
            s.no_copy += no_copy;
        });
        self.hold("Running", None, interval)
    }

    /// What the dashboard shows.
    pub fn status(&self) -> YoutubeCheckStatus {
        let current = self.settings.current();
        let actions = &current.library_actions;
        let files = self.files.lock().clone();
        let (state, reason) = self.hold.lock().clone();
        let s = self.store.read(Clone::clone);
        let position = files.map_or(0, |files| first_after(&files, &s.cursor));
        YoutubeCheckStatus {
            state,
            reason,
            running: s.running,
            paused: s.paused,
            position,
            total: s.total,
            done: s.done,
            asked: s.asked,
            alike: s.alike,
            unsure: s.unsure,
            no_copy: s.no_copy,
            open: self.notices.open_count(NoticeOrigin::LibrarySweep),
            keeper: LibraryReviewSweepWorker::keeper(actions, current.subsonic.admin_username.as_deref()),
            per_hour: actions.effective_youtube_check_per_hour(),
            started_utc: s.started_utc,
            finished_utc: s.finished_utc,
        }
    }
}

/// `relative` under `root`, made a full path.
fn full_path(root: &str, relative: &str) -> String {
    get_full_path(&format!("{root}/{relative}"))
}

/// A binary search for the cursor in ordinal order: the index after the cursor, or where it would
/// go.
fn first_after(files: &[String], cursor: &str) -> usize {
    match files.binary_search_by(|file| compare_ordinal(file, cursor)) {
        Ok(index) => index + 1,
        Err(index) => index,
    }
}

#[cfg(test)]
#[path = "library_youtube_check_tests.rs"]
mod tests;
