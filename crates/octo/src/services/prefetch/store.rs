//! The songs fetched ahead: real FLAC copies in a folder of their own, outside the music folder,
//! each beside a `<file>.json` with what its download learned. The folder is the truth: the
//! index is read from it on first use (and again when the folder setting changes), and when a
//! song was last played is the time on its `.json`, so nothing else needs to be saved.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use octo_core::common::Clock;
use octo_core::fingerprint::{InconclusiveReason, VerificationResult, VerificationVerdict};
use octo_core::models::domain::Song;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// The folder the songs are kept in, read at every use so a changed setting is followed.
pub type FolderFn = Arc<dyn Fn() -> PathBuf + Send + Sync>;

/// A file played this recently is never deleted: it may be playing right now.
const IN_USE_FOR: Duration = Duration::minutes(15);

/// The extension of a song's facts beside it.
const SIDECAR_EXTENSION: &str = "json";

/// What a download that fetched a song ahead learned, which the heart that keeps it later needs:
/// the checks it passed, where it came from and what it was made from.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AheadSidecar {
    pub provider: String,
    pub external_id: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    /// The file's format, upper case ("FLAC").
    pub format: String,
    pub ready_at: Option<DateTime<Utc>>,
    pub transcoded_from: Option<String>,
    /// The source that brought it ("Soulseek", "SpotiFLAC"), when the backend named one.
    pub fetched_from: Option<String>,
    pub source_peer: Option<String>,
    pub source_file: Option<String>,
    pub verification: Option<VerificationMirror>,
}

impl AheadSidecar {
    /// The facts of a song that just landed at `path`.
    pub fn of(
        provider: &str,
        external_id: &str,
        path: &Path,
        song: &Song,
        fetched_from: Option<&str>,
        ready_at: DateTime<Utc>,
    ) -> Self {
        AheadSidecar {
            provider: provider.to_string(),
            external_id: external_id.to_string(),
            artist: song.artist.clone(),
            title: song.title.clone(),
            album: song.album.clone(),
            format: path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_uppercase())
                .unwrap_or_default(),
            ready_at: Some(ready_at),
            transcoded_from: song.transcoded_from.clone(),
            fetched_from: fetched_from.map(str::to_string),
            source_peer: song.source_peer.clone(),
            source_file: song.source_file.clone(),
            verification: song.verification.as_deref().map(VerificationMirror::of),
        }
    }
}

/// The parts of AcoustID's verdict that outlive the download: what decides whether a person is
/// asked to review the song, what they are told, and what their answer is sent back with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VerificationMirror {
    pub verdict: String,
    pub reason: String,
    pub score: f64,
    pub threshold: f64,
    pub matched_title: Option<String>,
    pub matched_artist: Option<String>,
    pub matched_album: Option<String>,
    pub matched_year: Option<i32>,
    pub recording_id: Option<String>,
    pub deny_reason: String,
    pub tags_authoritative: bool,
    pub fingerprint: Option<String>,
    pub duration_seconds: i32,
    pub acoust_id: Option<String>,
    pub candidate_recording_id: Option<String>,
    pub evidence: Option<String>,
}

impl VerificationMirror {
    pub fn of(result: &VerificationResult) -> Self {
        VerificationMirror {
            verdict: result.verdict.name().to_string(),
            reason: result.reason.name().to_string(),
            score: result.score,
            threshold: result.threshold,
            matched_title: result.matched_title.clone(),
            matched_artist: result.matched_artist.clone(),
            matched_album: result.matched_album.clone(),
            matched_year: result.matched_year,
            recording_id: result.recording_id.clone(),
            deny_reason: result.deny_reason.clone(),
            tags_authoritative: result.tags_authoritative,
            fingerprint: result.fingerprint.clone(),
            duration_seconds: result.duration_seconds,
            acoust_id: result.acoust_id.clone(),
            candidate_recording_id: result.candidate_recording_id.clone(),
            evidence: result.evidence.clone(),
        }
    }

    /// The verdict again, without the whole AcoustID answer it was weighed from.
    pub fn to_result(&self) -> VerificationResult {
        let verdict = [
            VerificationVerdict::Confirmed,
            VerificationVerdict::Inconclusive,
            VerificationVerdict::Mismatch,
        ]
        .into_iter()
        .find(|v| v.name() == self.verdict)
        .unwrap_or_default();
        let reason = InconclusiveReason::ALL
            .into_iter()
            .find(|r| r.name() == self.reason)
            .unwrap_or_default();
        VerificationResult {
            verdict,
            reason,
            score: self.score,
            threshold: self.threshold,
            matched_title: self.matched_title.clone(),
            matched_artist: self.matched_artist.clone(),
            matched_album: self.matched_album.clone(),
            matched_year: self.matched_year,
            recording_id: self.recording_id.clone(),
            deny_reason: self.deny_reason.clone(),
            tags_authoritative: self.tags_authoritative,
            fingerprint: self.fingerprint.clone(),
            duration_seconds: self.duration_seconds,
            acoust_id: self.acoust_id.clone(),
            candidate_recording_id: self.candidate_recording_id.clone(),
            evidence: self.evidence.clone(),
            ..Default::default()
        }
    }
}

/// A song taken out of the folder by the heart that keeps it.
#[derive(Debug)]
pub struct Taken {
    pub path: PathBuf,
    pub sidecar: AheadSidecar,
    /// The whole verdict, while Octo has not restarted since the song was fetched.
    pub verification: Option<Box<VerificationResult>>,
}

impl Taken {
    /// The verdict the song was fetched with: the whole one when still known, else the one its
    /// facts kept.
    pub fn verification(&self) -> Option<Box<VerificationResult>> {
        self.verification.clone().or_else(|| {
            self.sidecar
                .verification
                .as_ref()
                .map(|mirror| Box::new(mirror.to_result()))
        })
    }
}

/// What the download base asks of the songs fetched ahead.
pub trait PrefetchedFiles: Send + Sync {
    /// Takes the song's file out of the folder for good, with its facts, or None when there is
    /// none. Its `.json` is gone once this returns, and nothing else can take or delete it.
    fn take(&self, provider: &str, external_id: &str) -> Option<Taken>;

    /// A download put the song at `path`, its facts beside it.
    fn kept(
        &self,
        provider: &str,
        external_id: &str,
        path: &Path,
        verification: Option<Box<VerificationResult>>,
    );
}

/// The file name a song is kept under: `<provider>_<id>.<ext>`, with anything but letters,
/// digits, `-` and `_` in the id replaced.
pub fn file_name(provider: &str, external_id: &str, extension: &str) -> String {
    let safe = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
    };
    let extension = extension.trim_start_matches('.');
    format!("{}_{}.{}", safe(provider), safe(external_id), safe(extension))
}

/// Where a song's facts are kept: `<file>.json`.
pub fn sidecar_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_os_string();
    name.push(".");
    name.push(SIDECAR_EXTENSION);
    PathBuf::from(name)
}

/// Writes a song's facts beside it.
pub fn write_sidecar(file: &Path, sidecar: &AheadSidecar) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(sidecar).map_err(std::io::Error::other)?;
    std::fs::write(sidecar_path(file), json)
}

fn key(provider: &str, external_id: &str) -> String {
    format!("{provider}:{external_id}")
}

#[derive(Debug, Clone)]
struct Entry {
    path: PathBuf,
    size: u64,
    last_used: DateTime<Utc>,
    ready_at: DateTime<Utc>,
    artist: String,
    title: String,
    verification: Option<Box<VerificationResult>>,
}

#[derive(Default)]
struct State {
    /// The folder the index was read from, once it has been.
    read_from: Option<PathBuf>,
    entries: HashMap<String, Entry>,
}

/// One song ready in the folder, for the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadySong {
    pub key: String,
    pub artist: String,
    pub title: String,
    pub size: u64,
}

/// The index of songs fetched ahead.
pub struct PrefetchStore {
    folder: FolderFn,
    clock: Clock,
    state: Mutex<State>,
}

impl PrefetchStore {
    pub fn new(folder: FolderFn, clock: Clock) -> Self {
        PrefetchStore {
            folder,
            clock,
            state: Mutex::new(State::default()),
        }
    }

    /// The folder the songs are kept in now.
    pub fn folder(&self) -> PathBuf {
        (self.folder)()
    }

    /// The state, read from the folder first when it has not been yet, or the folder changed.
    fn current(&self) -> parking_lot::MutexGuard<'_, State> {
        let folder = self.folder();
        let mut state = self.state.lock();
        if state.read_from.as_deref() != Some(folder.as_path()) {
            state.entries = read_folder(&folder);
            state.read_from = Some(folder);
            if !state.entries.is_empty() {
                info!(
                    "Songs downloaded ahead ready in {}: {}",
                    state.read_from.as_deref().unwrap_or(Path::new("")).display(),
                    state.entries.len()
                );
            }
        }
        state
    }

    /// The ready file of a song, or None when there is none.
    pub fn ready(&self, provider: &str, external_id: &str) -> Option<PathBuf> {
        let mut state = self.current();
        let key = key(provider, external_id);
        let path = state.entries.get(&key)?.path.clone();
        if path.is_file() {
            return Some(path);
        }
        // Removed from outside Octo: forgotten.
        state.entries.remove(&key);
        None
    }

    /// Whether the song is ready.
    pub fn is_ready(&self, provider: &str, external_id: &str) -> bool {
        self.ready(provider, external_id).is_some()
    }

    /// The song was played: it is kept as long as the newest songs.
    pub fn touch(&self, provider: &str, external_id: &str) {
        let now = self.clock.now();
        let path = {
            let mut state = self.current();
            let Some(entry) = state.entries.get_mut(&key(provider, external_id)) else {
                return;
            };
            entry.last_used = now;
            entry.path.clone()
        };
        set_modified(&sidecar_path(&path), now);
    }

    /// Adds a song whose file and facts are already in the folder, unless it is known.
    pub fn adopt(&self, path: &Path) {
        let mut state = self.current();
        if let Some((key, entry)) = read_entry(path)
            && !state.entries.contains_key(&key)
        {
            state.entries.insert(key, entry);
        }
    }

    /// How many songs are ready, and their size in bytes.
    pub fn sizes(&self) -> (usize, u64) {
        let state = self.current();
        (state.entries.len(), state.entries.values().map(|e| e.size).sum())
    }

    /// Every song ready, newest first.
    pub fn songs(&self) -> Vec<ReadySong> {
        let state = self.current();
        let mut songs: Vec<(&String, &Entry)> = state.entries.iter().collect();
        songs.sort_by(|a, b| b.1.ready_at.cmp(&a.1.ready_at).then_with(|| a.0.cmp(b.0)));
        songs
            .into_iter()
            .map(|(key, entry)| ReadySong {
                key: key.clone(),
                artist: entry.artist.clone(),
                title: entry.title.clone(),
                size: entry.size,
            })
            .collect()
    }

    /// Deletes every song in the folder but those played in the last 15 minutes. Answers how
    /// many went.
    pub fn clear(&self) -> usize {
        let now = self.clock.now();
        let mut state = self.current();
        let gone: Vec<String> = state
            .entries
            .iter()
            .filter(|(_, entry)| now - entry.last_used >= IN_USE_FOR)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &gone {
            if let Some(entry) = state.entries.remove(key) {
                delete(&entry.path);
            }
        }
        gone.len()
    }

    /// Deletes the songs nobody played for `keep_days`, then the least lately played until the
    /// folder holds at most `cap_bytes`. A song played in the last 15 minutes always stays.
    /// Answers how many went.
    pub fn sweep(&self, keep_days: i64, cap_bytes: u64) -> usize {
        let now = self.clock.now();
        let mut state = self.current();
        let mut by_use: Vec<(String, DateTime<Utc>, u64)> = state
            .entries
            .iter()
            .map(|(key, entry)| (key.clone(), entry.last_used, entry.size))
            .collect();
        by_use.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        let mut total: u64 = by_use.iter().map(|(_, _, size)| size).sum();
        let mut gone = 0;
        for (key, last_used, size) in by_use {
            if now - last_used < IN_USE_FOR {
                continue;
            }
            let stale = now - last_used >= Duration::days(keep_days);
            if !stale && total <= cap_bytes {
                continue;
            }
            if let Some(entry) = state.entries.remove(&key) {
                delete(&entry.path);
                total = total.saturating_sub(size);
                gone += 1;
            }
        }
        if gone > 0 {
            info!("Deleted {gone} songs downloaded ahead that were not played");
        }
        gone
    }

    /// Whether the folder holds more than `cap_bytes`.
    pub fn over(&self, cap_bytes: u64) -> bool {
        self.sizes().1 > cap_bytes
    }
}

impl PrefetchedFiles for PrefetchStore {
    fn take(&self, provider: &str, external_id: &str) -> Option<Taken> {
        let mut state = self.current();
        let entry = state.entries.remove(&key(provider, external_id))?;
        if !entry.path.is_file() {
            return None;
        }
        let side = sidecar_path(&entry.path);
        let sidecar = std::fs::read(&side)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AheadSidecar>(&bytes).ok())
            .unwrap_or_default();
        let _ = std::fs::remove_file(&side);
        Some(Taken {
            path: entry.path,
            sidecar,
            verification: entry.verification,
        })
    }

    fn kept(
        &self,
        provider: &str,
        external_id: &str,
        path: &Path,
        verification: Option<Box<VerificationResult>>,
    ) {
        let mut state = self.current();
        // Read back from the folder, so what is indexed is what a restart would find.
        let Some((_, mut entry)) = read_entry(path) else {
            warn!(
                "A song downloaded ahead is not where it was put: {}",
                path.display()
            );
            return;
        };
        entry.verification = verification;
        // Ready now counts as played now: a new song is kept as long as the newest.
        let now = self.clock.now();
        entry.last_used = now;
        set_modified(&sidecar_path(path), now);
        state.entries.insert(key(provider, external_id), entry);
    }
}

/// The index of a folder: every song with its facts beside it. A file without facts is one whose
/// download never finished, and facts without a file belong to a song already gone; both are
/// deleted.
fn read_folder(folder: &Path) -> HashMap<String, Entry> {
    let mut entries = HashMap::new();
    let Ok(listing) = std::fs::read_dir(folder) else {
        return entries;
    };
    let files: Vec<PathBuf> = listing
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    for path in &files {
        let is_sidecar = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(SIDECAR_EXTENSION));
        if is_sidecar {
            let audio = path.with_extension("");
            if !audio.is_file() {
                debug!(
                    "Removing facts of a song no longer downloaded ahead: {}",
                    path.display()
                );
                let _ = std::fs::remove_file(path);
            }
            continue;
        }
        match read_entry(path) {
            Some((key, entry)) => {
                entries.insert(key, entry);
            }
            // Only a file named the way songs are kept here is Octo's to remove.
            None if is_kept_name(path) => {
                info!(
                    "Removing a song whose download ahead never finished: {}",
                    path.display()
                );
                let _ = std::fs::remove_file(path);
            }
            None => {}
        }
    }
    entries
}

/// Whether a file is named the way songs are kept here: `<provider>_<id>.<audio extension>`.
fn is_kept_name(path: &Path) -> bool {
    let named = path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains('_'));
    let audio = path.extension().is_some_and(|e| {
        ["flac", "mp3", "m4a", "ogg", "opus", "wav"]
            .iter()
            .any(|known| e.eq_ignore_ascii_case(known))
    });
    named && audio
}

/// One song from its file and its facts, or None when its facts are missing or unreadable.
fn read_entry(path: &Path) -> Option<(String, Entry)> {
    let side = sidecar_path(path);
    let sidecar: AheadSidecar = serde_json::from_slice(&std::fs::read(&side).ok()?).ok()?;
    if sidecar.provider.is_empty() || sidecar.external_id.is_empty() {
        return None;
    }
    let size = std::fs::metadata(path).ok()?.len();
    let last_used = std::fs::metadata(&side)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now());
    let ready_at = sidecar.ready_at.unwrap_or(last_used);
    Some((
        key(&sidecar.provider, &sidecar.external_id),
        Entry {
            path: path.to_path_buf(),
            size,
            last_used,
            ready_at,
            artist: sidecar.artist,
            title: sidecar.title,
            verification: None,
        },
    ))
}

fn delete(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        warn!("Could not delete {}: {e}", path.display());
    }
    let _ = std::fs::remove_file(sidecar_path(path));
}

fn set_modified(path: &Path, at: DateTime<Utc>) {
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_modified(SystemTime::from(at));
    }
}

/// Free bytes on the disk that holds `folder` (or its nearest folder that exists), or None when
/// it cannot be read.
pub fn free_bytes(folder: &Path) -> Option<u64> {
    let existing = folder.ancestors().find(|dir| dir.is_dir())?;
    disk_free(existing)
}

#[cfg(unix)]
fn disk_free(dir: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(dir).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(not(unix))]
fn disk_free(_: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct World {
        dir: tempfile::TempDir,
        now: Arc<Mutex<DateTime<Utc>>>,
        store: PrefetchStore,
    }

    fn world() -> World {
        let dir = tempfile::tempdir().expect("a temp dir");
        let now = Arc::new(Mutex::new(Utc::now()));
        let folder = dir.path().join("prefetch");
        std::fs::create_dir_all(&folder).expect("the folder");
        let clock_now = Arc::clone(&now);
        let store = PrefetchStore::new(
            Arc::new(move || folder.clone()),
            Clock::new(move || *clock_now.lock()),
        );
        World { dir, now, store }
    }

    impl World {
        fn folder(&self) -> PathBuf {
            self.dir.path().join("prefetch")
        }

        /// A song as a download leaves it: the file, then its facts.
        fn put(&self, id: &str, bytes: usize) -> PathBuf {
            let path = self.folder().join(file_name("soulseek", id, "flac"));
            std::fs::write(&path, vec![0u8; bytes]).expect("the file");
            let song = Song {
                artist: "Artist".into(),
                title: format!("Song {id}"),
                transcoded_from: None,
                verification: Some(Box::new(VerificationResult {
                    verdict: VerificationVerdict::Inconclusive,
                    reason: InconclusiveReason::NoEntry,
                    duration_seconds: 201,
                    ..Default::default()
                })),
                ..Default::default()
            };
            let sidecar = AheadSidecar::of("soulseek", id, &path, &song, Some("SpotiFLAC"), *self.now.lock());
            write_sidecar(&path, &sidecar).expect("the facts");
            self.store.kept("soulseek", id, &path, None);
            path
        }

        fn later(&self, by: Duration) {
            *self.now.lock() += by;
        }
    }

    #[test]
    fn the_index_is_read_from_the_folder_and_half_written_songs_are_removed() {
        let w = world();
        let kept = w.put("a", 10);
        let partial = w.folder().join(file_name("soulseek", "b", "flac"));
        std::fs::write(&partial, [1, 2, 3]).expect("a partial file");
        let orphan = sidecar_path(&w.folder().join(file_name("soulseek", "c", "flac")));
        std::fs::write(&orphan, b"{}").expect("orphan facts");

        // A store started over the same folder, as after a restart.
        let folder = w.folder();
        let fresh = PrefetchStore::new(Arc::new(move || folder.clone()), Clock::system());
        assert_eq!(fresh.ready("soulseek", "a"), Some(kept.clone()));
        assert_eq!(fresh.sizes(), (1, 10));
        assert!(!partial.exists(), "a file with no facts never finished");
        assert!(!orphan.exists(), "facts with no file belong to nothing");
        assert!(fresh.ready("soulseek", "b").is_none());
    }

    #[test]
    fn a_file_not_named_as_songs_are_kept_is_left_alone() {
        let w = world();
        let theirs = w.folder().join("Notes.txt");
        let song = w.folder().join("01 Song.flac");
        std::fs::write(&theirs, b"mine").expect("a file");
        std::fs::write(&song, b"mine").expect("a file");
        let folder = w.folder();
        let fresh = PrefetchStore::new(Arc::new(move || folder.clone()), Clock::system());
        assert_eq!(fresh.sizes(), (0, 0));
        assert!(theirs.exists() && song.exists());
    }

    #[test]
    fn a_taken_song_leaves_the_index_with_its_facts() {
        let w = world();
        let path = w.put("a", 10);
        let taken = w.store.take("soulseek", "a").expect("taken");
        assert_eq!(taken.path, path);
        assert_eq!(taken.sidecar.fetched_from.as_deref(), Some("SpotiFLAC"));
        assert_eq!(taken.sidecar.format, "FLAC");
        let verdict = taken.verification().expect("the verdict");
        assert!(verdict.needs_review());
        assert_eq!(verdict.duration_seconds, 201);
        assert!(!sidecar_path(&path).exists());
        assert!(w.store.ready("soulseek", "a").is_none());
        assert!(w.store.take("soulseek", "a").is_none(), "taken once");
    }

    #[test]
    fn the_sweep_deletes_unplayed_songs_after_keep_days() {
        let w = world();
        let old = w.put("old", 10);
        w.later(Duration::days(20));
        let young = w.put("young", 10);
        w.later(Duration::days(11));
        assert_eq!(w.store.sweep(30, u64::MAX), 1);
        assert!(!old.exists() && !sidecar_path(&old).exists());
        assert!(young.exists());
    }

    #[test]
    fn the_sweep_deletes_the_least_lately_played_until_under_the_cap() {
        let w = world();
        let first = w.put("first", 100);
        w.later(Duration::hours(1));
        let second = w.put("second", 100);
        w.later(Duration::hours(1));
        let third = w.put("third", 100);
        w.later(Duration::hours(1));
        // Played again, so it is the newest.
        w.store.touch("soulseek", "first");
        w.later(Duration::hours(1));
        assert_eq!(w.store.sweep(30, 200), 1);
        assert!(first.exists() && third.exists());
        assert!(!second.exists());
        assert_eq!(w.store.sizes(), (2, 200));
    }

    #[test]
    fn a_song_played_in_the_last_fifteen_minutes_is_never_deleted() {
        let w = world();
        let playing = w.put("playing", 100);
        w.later(Duration::days(40));
        w.store.touch("soulseek", "playing");
        w.later(Duration::minutes(5));
        assert_eq!(w.store.sweep(30, 0), 0);
        assert_eq!(w.store.clear(), 0);
        assert!(playing.exists());
        w.later(Duration::minutes(15));
        assert_eq!(w.store.sweep(30, 0), 1);
        assert!(!playing.exists());
    }

    #[test]
    fn when_a_song_was_played_survives_a_restart() {
        let w = world();
        w.put("a", 10);
        w.later(Duration::days(29));
        w.store.touch("soulseek", "a");
        let folder = w.folder();
        let now = Arc::clone(&w.now);
        let fresh = PrefetchStore::new(Arc::new(move || folder.clone()), Clock::new(move || *now.lock()));
        w.later(Duration::days(2));
        assert_eq!(fresh.sweep(30, u64::MAX), 0, "played two days ago");
    }

    #[test]
    fn clear_deletes_every_song() {
        let w = world();
        let a = w.put("a", 10);
        let b = w.put("b", 10);
        w.later(Duration::hours(1));
        assert_eq!(w.store.clear(), 2);
        assert!(!a.exists() && !b.exists());
        assert_eq!(w.store.sizes(), (0, 0));
    }

    #[test]
    fn a_changed_folder_is_read_anew() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let which = Arc::new(Mutex::new(dir.path().join("one")));
        for name in ["one", "two"] {
            std::fs::create_dir_all(dir.path().join(name)).expect("a folder");
        }
        let path = dir.path().join("two").join(file_name("soulseek", "x", "flac"));
        std::fs::write(&path, [0; 4]).expect("the file");
        write_sidecar(
            &path,
            &AheadSidecar {
                provider: "soulseek".into(),
                external_id: "x".into(),
                ..Default::default()
            },
        )
        .expect("the facts");
        let folder = Arc::clone(&which);
        let store = PrefetchStore::new(Arc::new(move || folder.lock().clone()), Clock::system());
        assert!(store.ready("soulseek", "x").is_none());
        *which.lock() = dir.path().join("two");
        assert_eq!(store.ready("soulseek", "x"), Some(path));
    }

    #[test]
    fn names_are_safe_for_any_id() {
        assert_eq!(file_name("soulseek", "a/b c", ".flac"), "soulseek_a-b-c.flac");
        assert_eq!(
            sidecar_path(Path::new("/p/soulseek_x.flac")),
            PathBuf::from("/p/soulseek_x.flac.json")
        );
    }

    #[test]
    fn the_free_space_of_a_folder_not_made_yet_is_its_disk_s() {
        let dir = tempfile::tempdir().expect("a temp dir");
        if cfg!(unix) {
            assert!(free_bytes(&dir.path().join("not/yet")).is_some_and(|free| free > 0));
        }
    }
}
