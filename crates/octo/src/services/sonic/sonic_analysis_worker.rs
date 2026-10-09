//! The sonic analysis worker: Sounds alike (multi-source radio) asks octo-sonic about every
//! library song once, one at a time with a pause between, only while nothing downloads, then
//! keeps up with new and changed songs on a pass every [`PASS_INTERVAL`]. A song that arrived
//! through a tracked download goes first.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use futures::FutureExt;
use octo_core::common::Clock;
use octo_core::json::datetime;
use octo_core::library::generated_playlist_service::dotnet_ticks;
use octo_core::settings::SettingsStore;
use parking_lot::Mutex;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use super::sonic_client::{ISonicClient, SonicAnswer};
use super::sonic_store::{SonicFailure, SonicSong, SonicStore};
use crate::services::common::{AcquisitionTracker, IAcquisitionActivity};
use crate::services::library::navidrome_song_path_resolver::{Candidate, PathSource};
use crate::services::library::quality_upgrade_worker::LibrarySongRow;
use crate::services::library::{NavidromePlaylistApi, NavidromeSongPathResolver};

/// A local file of a library song, as the analysis stamps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SonicFile {
    /// The full path.
    pub full_name: String,
    /// The size in bytes.
    pub length: i64,
    /// The last write time, in 100-nanosecond ticks ([`dotnet_ticks`]).
    pub last_write_ticks: i64,
}

impl SonicFile {
    /// The file as it is on disk now; None when it cannot be read.
    pub fn stat(path: &str) -> Option<SonicFile> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified: DateTime<Utc> = metadata.modified().ok()?.into();
        Some(SonicFile {
            full_name: path.to_string(),
            length: metadata.len() as i64,
            last_write_ticks: dotnet_ticks(modified),
        })
    }
}

/// What the analysis reads of the library: Navidrome's song list, a song that just arrived, and
/// the local file of a song. A trait so the worker can be tested without one.
#[async_trait]
pub trait ISonicLibrary: Send + Sync {
    /// One page of the song list, and how many rows the page had before missing and pathless
    /// songs were left out. None when the page could not be read.
    async fn list(&self, start: i32, count: i32) -> Option<(Vec<LibrarySongRow>, usize)>;
    async fn arrived(&self, id: &str) -> Option<LibrarySongRow>;
    fn local_file(&self, row: &LibrarySongRow) -> Option<SonicFile>;
}

/// The library as Navidrome lists it, with each file checked where Octo can read it.
pub struct NavidromeSonicLibrary {
    api: Arc<NavidromePlaylistApi>,
    resolver: Arc<NavidromeSongPathResolver>,
}

impl NavidromeSonicLibrary {
    pub fn new(api: Arc<NavidromePlaylistApi>, resolver: Arc<NavidromeSongPathResolver>) -> Self {
        NavidromeSonicLibrary { api, resolver }
    }
}

#[async_trait]
impl ISonicLibrary for NavidromeSonicLibrary {
    async fn list(&self, start: i32, count: i32) -> Option<(Vec<LibrarySongRow>, usize)> {
        self.api.list_songs(start, count).await
    }

    async fn arrived(&self, id: &str) -> Option<LibrarySongRow> {
        let file = self.resolver.resolve(id).await?;
        Some(LibrarySongRow {
            id: id.to_string(),
            path: file.absolute_path,
            library_path: None,
            size: file.size_bytes,
            suffix: file.suffix,
            bit_rate: 0,
            title: file.title,
            artist: file.artist,
            duration: file.duration_seconds,
            album: Some(file.album),
            ..Default::default()
        })
    }

    /// The verified local file of a song from Navidrome's song list; None when Octo cannot read
    /// it here (Sounds alike sends only files it can name to octo-sonic).
    fn local_file(&self, row: &LibrarySongRow) -> Option<SonicFile> {
        let candidate = Candidate {
            id: row.id.clone(),
            raw_path: Some(row.path.clone()),
            library_path: row.library_path.clone(),
            size: row.size,
            title: row.title.clone(),
            artist: row.artist.clone(),
            album: row.album.clone().unwrap_or_default(),
            suffix: row.suffix.clone(),
            duration: row.duration,
            source: PathSource::NativeApi,
            missing: false,
            album_artist: row.album_artist.clone(),
        };
        let resolved = self
            .resolver
            .verify(&candidate, &self.resolver.music_root(), true)?;
        SonicFile::stat(&resolved.absolute_path)
    }
}

/// Where the analysis is, for the dashboard. Skipped: songs this pass leaves out, as Octo cannot
/// find their file, or they are too long or have no length.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SonicStatus {
    pub state: String,
    pub reason: Option<String>,
    /// Spelled "analysed" because that is the JSON field's name, and a JSON field keeps its name.
    pub analysed: i32,
    pub total: i32,
    pub failed: i32,
    pub pass: i32,
    #[serde(with = "datetime::utc_option")]
    pub next_pass_utc: Option<DateTime<Utc>>,
    pub paused: bool,
    pub skipped: i32,
}

pub const IDLE_CHECK: Duration = Duration::from_secs(60);
pub const BUSY_CHECK: Duration = Duration::from_secs(30);
pub const DOWN_CHECK: Duration = Duration::from_secs(10 * 60);
pub const PASS_INTERVAL: TimeDelta = TimeDelta::hours(6);
pub const RETRY_FAILED_AFTER: TimeDelta = TimeDelta::days(7);
pub const PAGE_SIZE: i32 = 1000;
/// Times octo-sonic dropped the connection on one song before it is marked unreadable.
pub const DROPS_BEFORE_FAILED: i32 = 2;
/// Longer than radio ever plays (RadioFiller), so not worth reading over the network.
pub const LONGEST_SECONDS: i32 = 45 * 60;

pub const TURNED_OFF_IN_ENV: &str =
    "Sounds alike is off in .env (RADIO_SOUNDS_ALIKE=false), so octo-sonic reads nothing.";
pub const CANNOT_SEE_MUSIC: &str = "octo-sonic cannot see the music folder. Check that it mounts the same folder as Octo (DOWNLOAD_PATH) and can read it (SONIC_USER).";
const DONE: &str = "Every song has been analyzed. New or changed ones are looked for later.";

/// This pass's list and what it has learned on the way.
#[derive(Default)]
struct Pass {
    rows: Option<Vec<LibrarySongRow>>,
    /// Whether this pass's list is the whole library: only then are songs no longer in it
    /// forgotten, so one failed page never costs the songs after it.
    complete: bool,
    /// Songs this pass leaves out, so each tick does not look for their files again.
    skipped: HashSet<String>,
    /// Times octo-sonic dropped the connection on a song, by id. Twice means the file crashes
    /// it; it is then marked unreadable instead of being tried forever.
    dropped: HashMap<String, i32>,
}

/// The next song to read: its row, its file and the file's stamp.
type Target = (LibrarySongRow, String, String);

pub struct SonicAnalysisWorker {
    store: Arc<SonicStore>,
    sonic: Arc<dyn ISonicClient>,
    library: Arc<dyn ISonicLibrary>,
    activity: Arc<dyn IAcquisitionActivity>,
    /// The radio source settings.
    settings: Arc<SettingsStore>,
    clock: Clock,
    first: Mutex<VecDeque<String>>,
    pass: Mutex<Pass>,
    generation: AtomicU64,
    /// State and reason for the dashboard. Until the first tick, which waits for start-up to
    /// finish: not "Off", which would mislead.
    status: Mutex<(String, Option<String>)>,
}

impl SonicAnalysisWorker {
    pub fn new(
        store: Arc<SonicStore>,
        sonic: Arc<dyn ISonicClient>,
        library: Arc<dyn ISonicLibrary>,
        activity: Arc<dyn IAcquisitionActivity>,
        settings: Arc<SettingsStore>,
        tracker: Option<&Arc<AcquisitionTracker>>,
        clock: Clock,
    ) -> Arc<Self> {
        let worker = Arc::new(SonicAnalysisWorker {
            store,
            sonic,
            library,
            activity,
            settings,
            clock,
            first: Mutex::new(VecDeque::new()),
            pass: Mutex::new(Pass::default()),
            generation: AtomicU64::new(0),
            status: Mutex::new((
                "Waiting".to_string(),
                Some("Starts a couple of minutes after Octo does.".to_string()),
            )),
        });
        if let Some(tracker) = tracker {
            let weak: Weak<SonicAnalysisWorker> = Arc::downgrade(&worker);
            tracker.subscribe_ended(Arc::new(move |end| {
                if let Some(worker) = weak.upgrade()
                    && end.done
                    && let Some(id) = end.library_id.as_deref().filter(|id| !id.is_empty())
                {
                    worker.arrived_first(id);
                }
            }));
        }
        worker
    }

    pub fn store(&self) -> &Arc<SonicStore> {
        &self.store
    }

    /// A song that just arrived in the library is analyzed before the rest of the pass.
    pub fn arrived_first(&self, library_id: &str) {
        self.first.lock().push_back(library_id.to_string());
    }

    pub async fn run(self: Arc<Self>, stopping: CancellationToken) -> anyhow::Result<()> {
        // Never alongside startup, which already works the disk and Navidrome.
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(120)) => {}
            _ = stopping.cancelled() => {
                self.store.flush();
                return Ok(());
            }
        }
        while !stopping.is_cancelled() {
            // Per-tick catch is mandatory: an unhandled exception would stop the host.
            let tick = tokio::select! {
                tick = std::panic::AssertUnwindSafe(self.tick()).catch_unwind() => tick,
                _ = stopping.cancelled() => break,
            };
            let wait = tick.unwrap_or_else(|_| {
                error!("Sounds alike analysis failed");
                IDLE_CHECK
            });
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stopping.cancelled() => break,
            }
        }
        self.store.flush();
        Ok(())
    }

    pub async fn tick(&self) -> Duration {
        let settings = self.settings.current().radio_sources.clone();
        if !settings.sounds_alike {
            return self.hold("Off", Some("Sounds alike is off on the Radio page."), IDLE_CHECK);
        }
        if self.store.read(|s| s.paused) {
            return self.hold("Paused", Some("Paused from the dashboard."), IDLE_CHECK);
        }
        let Some(health) = self.sonic.health().await else {
            return self.hold("Waiting", Some("octo-sonic is not answering."), DOWN_CHECK);
        };
        if health.off {
            return self.hold("Off", Some(TURNED_OFF_IN_ENV), IDLE_CHECK);
        }
        if health.problem.is_some() {
            return self.hold("Waiting", Some(CANNOT_SEE_MUSIC), DOWN_CHECK);
        }
        if self.activity.is_busy() {
            return self.hold("Waiting", Some("Waiting for a download to finish."), BUSY_CHECK);
        }

        let now = self.clock.now();
        // Between passes only the songs that just arrived are read, without listing the library
        // again or counting a pass.
        let arrivals_only = self.pass.lock().rows.is_none()
            && self
                .store
                .read(|s| s.next_pass_utc.is_some_and(|next| next > now));
        if arrivals_only && self.first.lock().is_empty() {
            return self.hold("Done", Some(DONE), IDLE_CHECK);
        }

        let generation = self.generation.load(Ordering::SeqCst);
        if !arrivals_only && self.pass.lock().rows.is_none() {
            let Some((rows, complete)) = self.list().await else {
                return self.hold(
                    "Waiting",
                    Some("Navidrome's song list could not be read (Octo needs its own Navidrome sign-in); trying again."),
                    IDLE_CHECK,
                );
            };
            if generation != self.generation.load(Ordering::SeqCst) {
                return Duration::ZERO;
            }
            let mut pass = self.pass.lock();
            pass.rows = Some(rows);
            pass.complete = complete;
            pass.skipped.clear();
        }

        let Some((row, file, stamp)) = self.next_target(health.version, now, arrivals_only).await else {
            if arrivals_only {
                return self.hold("Done", Some(DONE), IDLE_CHECK);
            }
            if generation != self.generation.load(Ordering::SeqCst) {
                return Duration::ZERO;
            }
            let (present, complete, count, skipped) = {
                let pass = self.pass.lock();
                let rows = pass.rows.as_deref().unwrap_or_default();
                (
                    rows.iter().map(|row| row.id.clone()).collect::<HashSet<String>>(),
                    pass.complete,
                    rows.len(),
                    pass.skipped.len(),
                )
            };
            self.store.write(|s| {
                if complete {
                    s.songs.retain(|id, _| present.contains(id));
                    s.failed.retain(|id, _| present.contains(id));
                }
                s.pass += 1;
                s.pass_finished_utc = Some(now);
                s.next_pass_utc = Some(now + PASS_INTERVAL);
            });
            self.store.flush();
            info!("Sounds alike finished a pass over {count} song(s), {skipped} left out");
            self.pass.lock().rows = None;
            return self.hold("Done", Some(DONE), IDLE_CHECK);
        };

        let answer = match self.sonic.analyze(&file).await {
            Ok(answer) => answer,
            Err(_) => {
                // The connection dropped mid-file: octo-sonic restarted, or the file crashed its
                // reader. Once is chance; twice on the same file, it is that file.
                let drops = {
                    let mut pass = self.pass.lock();
                    let drops = pass.dropped.entry(row.id.clone()).or_insert(0);
                    *drops += 1;
                    *drops
                };
                if drops < DROPS_BEFORE_FAILED {
                    return self.hold(
                        "Waiting",
                        Some("octo-sonic stopped answering; trying again."),
                        DOWN_CHECK,
                    );
                }
                self.pass.lock().dropped.remove(&row.id);
                SonicAnswer {
                    features: None,
                    error: Some("octo-sonic stopped while reading it".to_string()),
                    status: 0,
                }
            }
        };
        // An answer came, so octo-sonic is up: a drop counts only twice in a row.
        if answer.status != 0 {
            self.pass.lock().dropped.clear();
        }
        if generation != self.generation.load(Ordering::SeqCst) {
            return Duration::ZERO;
        }
        // Octo sees the file and octo-sonic does not: its music mount is not Octo's. That is no
        // fault of the song, so nothing is recorded and the song is asked again once it is fixed.
        if answer.status == 403 || answer.status == 404 {
            return self.hold("Waiting", Some(CANNOT_SEE_MUSIC), DOWN_CHECK);
        }
        let (features, error) = (answer.features, answer.error);
        if features.is_none() {
            info!(
                "octo-sonic could not read '{} - {}': {}",
                row.artist,
                row.title,
                error.as_deref().unwrap_or("")
            );
        }
        self.store.write(|s| match features {
            Some(features) => {
                s.songs.insert(
                    row.id.clone(),
                    SonicSong {
                        stamp,
                        title: row.title.clone(),
                        artist: row.artist.clone(),
                        album: row.album.clone(),
                        duration: row.duration,
                        version: features.version,
                        f: features.values,
                    },
                );
                s.failed.shift_remove(&row.id);
            }
            None => {
                s.failed.insert(
                    row.id.clone(),
                    SonicFailure {
                        stamp,
                        error: error.unwrap_or_else(|| "unknown".to_string()),
                        at_utc: now,
                    },
                );
            }
        });
        self.hold(
            "Running",
            None,
            Duration::from_secs(settings.effective_sonic_pause_seconds().max(0) as u64),
        )
    }

    /// A song first in line, else the next one with no features, a changed file, older features,
    /// or a failure old enough to try again.
    async fn next_target(&self, version: i32, now: DateTime<Utc>, arrivals_only: bool) -> Option<Target> {
        loop {
            let Some(id) = self.first.lock().pop_front() else {
                break;
            };
            // A song that just arrived is not in this pass's list yet: ask Navidrome for it, and
            // keep it in the list so the pass's end does not forget it again.
            let listed = if arrivals_only {
                None
            } else {
                self.pass
                    .lock()
                    .rows
                    .as_ref()
                    .and_then(|rows| rows.iter().find(|row| row.id == id).cloned())
            };
            let row = match listed {
                Some(row) => Some(row),
                None => match self.library.arrived(&id).await {
                    Some(arrived) => {
                        if !arrivals_only && let Some(rows) = self.pass.lock().rows.as_mut() {
                            rows.push(arrived.clone());
                        }
                        Some(arrived)
                    }
                    None => None,
                },
            };
            if let Some(row) = row
                && let Some(found) = self.candidate(&row, version, now, &mut self.pass.lock().skipped)
            {
                return Some(found);
            }
        }
        if arrivals_only {
            return None;
        }
        // The rows are checked in place, under the pass's lock: the cheap checks first, and a
        // file's stat only for a song not already known.
        let mut pass = self.pass.lock();
        let Pass { rows, skipped, .. } = &mut *pass;
        for row in rows.as_deref()? {
            if skipped.contains(&row.id) {
                continue;
            }
            if let Some(found) = self.candidate(row, version, now, skipped) {
                return Some(found);
            }
        }
        None
    }

    fn candidate(
        &self,
        row: &LibrarySongRow,
        version: i32,
        now: DateTime<Utc>,
        skipped: &mut HashSet<String>,
    ) -> Option<Target> {
        // No length is a file Navidrome could not read, and a long one is more than radio plays.
        if !row.duration.is_some_and(|d| d > 0 && d <= LONGEST_SECONDS) {
            skipped.insert(row.id.clone());
            return None;
        }
        let (done, failed) = self
            .store
            .read(|s| (s.songs.get(&row.id).cloned(), s.failed.get(&row.id).cloned()));
        // Cheap checks first, without touching the disk (a stat over a network mount is slow):
        // a song already analyzed at this size and version, or one that failed at this size
        // lately.
        let size_prefix = format!("{}:", row.size);
        if done
            .as_ref()
            .is_some_and(|done| done.version == version && done.stamp.starts_with(&size_prefix))
        {
            return None;
        }
        if failed.as_ref().is_some_and(|lately| {
            lately.stamp.starts_with(&size_prefix) && now - lately.at_utc < RETRY_FAILED_AFTER
        }) {
            return None;
        }
        let Some(file) = self.library.local_file(row) else {
            skipped.insert(row.id.clone());
            return None;
        };
        let stamp = format!("{}:{}", file.length, file.last_write_ticks);
        if done.is_some_and(|same| same.version == version && same.stamp == stamp) {
            return None;
        }
        if failed.is_some_and(|failed| failed.stamp == stamp && now - failed.at_utc < RETRY_FAILED_AFTER) {
            return None;
        }
        Some((row.clone(), file.full_name, stamp))
    }

    /// The library, one song per id, and whether the list reached its end. None when any page
    /// could not be read: a list with a hole would look like songs gone.
    async fn list(&self) -> Option<(Vec<LibrarySongRow>, bool)> {
        let mut rows: Vec<LibrarySongRow> = Vec::new();
        for page in 0..200 {
            let (found, count) = self.library.list(page * PAGE_SIZE, PAGE_SIZE).await?;
            rows.extend(found);
            // Count is the page's rows before missing and pathless songs were left out: a page
            // shorter than asked is the end, a page with a missing song is not.
            if (count as i64) < i64::from(PAGE_SIZE) {
                return Some((distinct(rows), true));
            }
        }
        Some((distinct(rows), false))
    }

    pub fn set_paused(&self, paused: bool) {
        self.store.write(|s| s.paused = paused);
        self.store.flush();
    }

    /// Forget every song's features and start a new pass.
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        {
            let mut pass = self.pass.lock();
            pass.rows = None;
            pass.skipped.clear();
        }
        self.store.write(|s| {
            s.songs.clear();
            s.failed.clear();
            s.pass = 0;
            s.next_pass_utc = None;
            s.pass_finished_utc = None;
        });
        self.store.flush();
    }

    pub fn status(&self) -> SonicStatus {
        let (rows, skipped) = {
            let pass = self.pass.lock();
            // This pass's, or the last one's once it is done.
            (pass.rows.as_ref().map(Vec::len), pass.skipped.len())
        };
        let (state, reason) = self.status.lock().clone();
        self.store.read(|s| SonicStatus {
            state,
            reason,
            analysed: s.songs.len() as i32,
            total: rows.unwrap_or(s.songs.len() + s.failed.len()) as i32,
            failed: s.failed.len() as i32,
            pass: s.pass,
            next_pass_utc: s.next_pass_utc,
            paused: s.paused,
            skipped: skipped as i32,
        })
    }

    fn hold(&self, state: &str, reason: Option<&str>, wait: Duration) -> Duration {
        *self.status.lock() = (state.to_string(), reason.map(str::to_string));
        wait
    }
}

/// One row per id (compared exactly), the first kept.
fn distinct(rows: Vec<LibrarySongRow>) -> Vec<LibrarySongRow> {
    let mut seen = HashSet::new();
    rows.into_iter()
        .filter(|row| seen.insert(row.id.clone()))
        .collect()
}

#[cfg(test)]
#[path = "sonic_analysis_worker_tests.rs"]
mod tests;
