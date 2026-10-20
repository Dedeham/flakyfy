//! The listening log: every completed play of each person, for their Listening page.
//!
//! One file of JSON lines per person under `<config>/listening/` (`<user>.jsonl`), a play
//! appended as it is recorded; pruning, a heart that comes after the play was written and
//! clearing write the whole file again through a temporary file. `listening.json` beside them
//! keeps the names as first seen, whether the old radio history was copied in, and the genre
//! answers already looked up for artists and albums.
//!
//! Plays are kept for `Listening:KeepDays`, pruned when the log is read and once a day.
//! Nothing is written while `Listening:Enabled` is off.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use octo_core::common::Clock;
use octo_core::last_fm::last_fm_radio_state_store::user_key;
use octo_core::models::radio::LastFmRadioPlay;
use octo_core::settings::SettingsStore;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::services::state_file;

/// The app a play is filed under when nothing says which app played it.
pub const UNKNOWN_APP: &str = "Unknown app";
/// The platform of a play whose app did not say.
pub const UNKNOWN_PLATFORM: &str = "unknown";
/// The meta file beside the people's logs.
pub const META_FILE: &str = "listening.json";

/// How long after a now-playing a completed play may still be timed from it, past the song's
/// length.
const NOW_PLAYING_SLACK: TimeDelta = TimeDelta::minutes(5);
/// The longest a play with no known length is timed from its now-playing.
const UNTIMED_LIMIT: TimeDelta = TimeDelta::minutes(30);
/// A heart this long before a play started still counts for it (the app may have sent the
/// heart a moment before the now-playing).
const HEART_LEAD: TimeDelta = TimeDelta::seconds(30);
/// A heart this long after a play ended still counts for it.
const HEART_TRAIL: TimeDelta = TimeDelta::minutes(2);
/// How long a heart waits for its play to be recorded.
const HEART_WAIT: TimeDelta = TimeDelta::hours(2);
/// The most hearts kept waiting at once.
const HEARTS_KEPT: usize = 500;

/// One completed play.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Play {
    pub song_id: String,
    /// The id the song's cover is asked for by: the library's cover id, or the song's own id.
    pub cover_id: String,
    pub artist: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<String>,
    pub title: String,
    pub album: String,
    /// The genre, from the song, its album or its artist; None when none of them has one.
    pub genre: Option<String>,
    /// How long it played, in seconds.
    pub seconds: i64,
    pub is_local: bool,
    /// The song was hearted while it played.
    pub hearted: bool,
    /// The app, as the Family page names a device's app ("Octo 1.9.0 (Android)").
    pub app: String,
    /// android, iphone, windows, macos, linux or unknown.
    pub platform: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// When the play started.
    pub played_at: DateTime<Utc>,
    /// scrobble, internet-radio or backfill.
    pub source: String,
}

impl Default for Play {
    fn default() -> Self {
        Play {
            song_id: String::new(),
            cover_id: String::new(),
            artist: String::new(),
            album_artist: None,
            title: String::new(),
            album: String::new(),
            genre: None,
            seconds: 0,
            is_local: false,
            hearted: false,
            app: UNKNOWN_APP.to_string(),
            platform: UNKNOWN_PLATFORM.to_string(),
            device_id: None,
            played_at: DateTime::<Utc>::UNIX_EPOCH,
            source: "scrobble".to_string(),
        }
    }
}

impl Play {
    /// When the play ended.
    pub fn ended_at(&self) -> DateTime<Utc> {
        self.played_at + TimeDelta::seconds(self.seconds.max(0))
    }

    /// Whether a heart at `at` came while this play was on.
    fn heart_counts(&self, at: DateTime<Utc>) -> bool {
        at >= self.played_at - HEART_LEAD && at <= self.ended_at() + HEART_TRAIL
    }
}

/// The platform in a User-Agent's parentheses: `Octo/1.9.0 (Android)` is android. Android is
/// checked before Linux, since Android's own agents name both.
pub fn platform_of(user_agent: &str) -> &'static str {
    let inside = user_agent
        .split_once('(')
        .map(|(_, rest)| rest.split(')').next().unwrap_or(rest))
        .unwrap_or("")
        .to_ascii_lowercase();
    if inside.contains("android") {
        "android"
    } else if inside.contains("iphone") || inside.contains("ipad") || inside.contains("ios") {
        "iphone"
    } else if inside.contains("windows") {
        "windows"
    } else if inside.contains("mac") {
        "macos"
    } else if inside.contains("linux") {
        "linux"
    } else {
        UNKNOWN_PLATFORM
    }
}

/// What is kept beside the logs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Meta {
    version: i32,
    /// The radio history was copied in once.
    backfilled: bool,
    /// Each person's name as first seen, by key.
    names: HashMap<String, String>,
    /// The genre found for an artist (by key), or "" when it has none.
    artist_genres: HashMap<String, String>,
    /// The genre of a library album (by id), or "" when it has none.
    album_genres: HashMap<String, String>,
}

#[derive(Default)]
struct Loaded {
    meta: Meta,
    /// Each person's plays by key, oldest first.
    users: HashMap<String, Vec<Play>>,
    /// The song each person last said was playing, and when.
    now_playing: HashMap<String, (String, DateTime<Utc>)>,
    /// Hearts no recorded play has taken yet: person, song id, when.
    hearts: Vec<(String, String, DateTime<Utc>)>,
}

pub struct ListeningLog {
    dir: PathBuf,
    settings: Arc<SettingsStore>,
    clock: Clock,
    state: Mutex<Option<Loaded>>,
}

impl ListeningLog {
    pub fn new(dir: impl Into<PathBuf>, settings: Arc<SettingsStore>) -> Self {
        Self::with_clock(dir, settings, Clock::system())
    }

    /// The log on another clock, for tests.
    pub fn with_clock(dir: impl Into<PathBuf>, settings: Arc<SettingsStore>, clock: Clock) -> Self {
        ListeningLog {
            dir: dir.into(),
            settings,
            clock,
            state: Mutex::new(None),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Whether plays are being written down.
    pub fn recording(&self) -> bool {
        self.settings.current().listening.enabled
    }

    fn keep(&self) -> TimeDelta {
        TimeDelta::days(self.settings.current().listening.effective_keep_days())
    }

    /// `user` started playing `song_id` now.
    pub fn now_playing(&self, user: &str, song_id: &str) {
        let key = user_key(user);
        if key.is_empty() || song_id.is_empty() || !self.recording() {
            return;
        }
        let now = self.now();
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        state.now_playing.insert(key, (song_id.to_string(), now));
    }

    /// How long a play of `song_id` that just completed lasted: from that person's now-playing
    /// for the same song, when there is one recent enough, capped at the song's length; else
    /// the song's length (0 when that is not known either).
    pub fn listened_seconds(&self, user: &str, song_id: &str, duration: Option<i32>) -> i64 {
        let length = duration.filter(|d| *d > 0).map(i64::from);
        let now = self.now();
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        let started = state
            .now_playing
            .get(&user_key(user))
            .filter(|(id, _)| id == song_id)
            .map(|(_, at)| *at);
        let limit = length.map_or(UNTIMED_LIMIT, |l| TimeDelta::seconds(l) + NOW_PLAYING_SLACK);
        match started.map(|at| now - at) {
            Some(elapsed) if elapsed >= TimeDelta::zero() && elapsed <= limit => {
                let seconds = elapsed.num_seconds();
                length.map_or(seconds, |l| seconds.min(l))
            }
            _ => length.unwrap_or(0),
        }
    }

    /// Writes down a completed play of `user`'s. False when recording is off or the play names
    /// no song.
    pub fn record(&self, user: &str, mut play: Play) -> bool {
        let key = user_key(user);
        if key.is_empty()
            || play.title.trim().is_empty()
            || play.artist.trim().is_empty()
            || !self.recording()
        {
            return false;
        }
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        // A heart that came while it played.
        let before = state.hearts.len();
        state
            .hearts
            .retain(|(who, id, at)| !(who == &key && id == &play.song_id && play.heart_counts(*at)));
        if state.hearts.len() != before {
            play.hearted = true;
        }
        if !state.meta.names.contains_key(&key) {
            state.meta.names.insert(key.clone(), user.trim().to_string());
            self.save_meta(&state.meta);
        }
        let line = match serde_json::to_string(&play) {
            Ok(line) => line,
            Err(e) => {
                warn!("A play of {} could not be written down: {e}", play.title);
                return false;
            }
        };
        let rows = state.users.entry(key.clone()).or_default();
        let at = rows.partition_point(|row| row.played_at <= play.played_at);
        rows.insert(at, play);
        if let Err(e) = self.append(&key, &line) {
            warn!("The listening log for {key} could not be written: {e}");
        }
        true
    }

    /// `user` hearted these songs now: a play of one that is on, or just ended, is marked; one
    /// recorded later takes the heart if it came while it played.
    pub fn heart(&self, user: &str, song_ids: &[String]) {
        let key = user_key(user);
        if key.is_empty() || song_ids.is_empty() || !self.recording() {
            return;
        }
        let now = self.now();
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        let mut changed = false;
        for id in song_ids.iter().filter(|id| !id.is_empty()) {
            let marked = state.users.get_mut(&key).and_then(|rows| {
                rows.iter_mut()
                    .rev()
                    .take_while(|row| row.ended_at() + HEART_TRAIL >= now)
                    .find(|row| &row.song_id == id && row.heart_counts(now))
            });
            match marked {
                Some(row) => {
                    if !row.hearted {
                        row.hearted = true;
                        changed = true;
                    }
                }
                None => state.hearts.push((key.clone(), id.clone(), now)),
            }
        }
        state.hearts.retain(|(_, _, at)| now - *at <= HEART_WAIT);
        if state.hearts.len() > HEARTS_KEPT {
            let extra = state.hearts.len() - HEARTS_KEPT;
            state.hearts.drain(..extra);
        }
        if changed && let Some(rows) = state.users.get(&key) {
            self.rewrite(&key, rows);
        }
    }

    /// Everyone with plays on record, as (key, name), by name.
    pub fn people(&self) -> Vec<(String, String)> {
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        let mut people: Vec<(String, String)> = state
            .users
            .iter()
            .filter(|(_, rows)| !rows.is_empty())
            .map(|(key, _)| {
                let name = state.meta.names.get(key).cloned().unwrap_or_else(|| key.clone());
                (key.clone(), name)
            })
            .collect();
        people.sort_by_key(|a| a.1.to_lowercase());
        people
    }

    /// The name a person is shown by.
    pub fn name_of(&self, user: &str) -> String {
        let key = user_key(user);
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        state
            .meta
            .names
            .get(&key)
            .cloned()
            .unwrap_or_else(|| user.trim().to_string())
    }

    /// A copy of one person's plays, oldest first.
    pub fn plays_of(&self, user: &str) -> Vec<Play> {
        let key = user_key(user);
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        state.users.get(&key).cloned().unwrap_or_default()
    }

    /// A copy of everyone's plays, by key.
    pub fn all_plays(&self) -> Vec<(String, Vec<Play>)> {
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        state
            .users
            .iter()
            .map(|(key, rows)| (key.clone(), rows.clone()))
            .collect()
    }

    /// Removes one person's plays, or everyone's. Returns how many plays went.
    pub fn clear(&self, user: Option<&str>) -> usize {
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        let keys: Vec<String> = match user {
            Some(user) => vec![user_key(user)],
            None => state.users.keys().cloned().collect(),
        };
        let mut removed = 0;
        for key in keys {
            if let Some(rows) = state.users.remove(&key) {
                removed += rows.len();
            }
            state.now_playing.remove(&key);
            state.hearts.retain(|(who, _, _)| who != &key);
            match std::fs::remove_file(self.user_path(&key)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => warn!("The listening log for {key} could not be removed: {e}"),
            }
        }
        removed
    }

    /// Copies the radio's play history in, once ever: completed scrobbles and radio stream
    /// plays (never the radio's own starter seeds), each as played by an unknown app, as long
    /// as it is within the days kept. Returns how many plays were copied; 0 once it has run.
    pub fn backfill(&self, history: Vec<(String, Vec<LastFmRadioPlay>)>) -> usize {
        let since = self.now() - self.keep();
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        if state.meta.backfilled {
            return 0;
        }
        let mut copied = 0;
        for (user, plays) in history {
            let key = user_key(&user);
            if key.is_empty() {
                continue;
            }
            let rows = state.users.entry(key.clone()).or_default();
            let mut added = 0;
            for play in plays {
                if !matches!(play.source.as_str(), "scrobble" | "internet-radio")
                    || play.played_at_utc < since
                    || play.artist.trim().is_empty()
                    || play.title.trim().is_empty()
                {
                    continue;
                }
                let seconds = i64::from(play.duration.unwrap_or(0).max(0));
                // Radio history dates a play from when it was reported, at its end.
                let played_at = play.played_at_utc - TimeDelta::seconds(seconds);
                if rows.iter().any(|row| {
                    row.song_id == play.song_id && (row.played_at - played_at).abs() <= TimeDelta::minutes(5)
                }) {
                    continue;
                }
                rows.push(Play {
                    cover_id: play.song_id.clone(),
                    song_id: play.song_id,
                    artist: play.artist,
                    title: play.title,
                    album: play.album.unwrap_or_default(),
                    genre: play.genre.filter(|g| !g.trim().is_empty()),
                    seconds,
                    is_local: play.is_local,
                    hearted: false,
                    played_at,
                    source: "backfill".to_string(),
                    ..Default::default()
                });
                added += 1;
            }
            if added == 0 {
                if rows.is_empty() {
                    state.users.remove(&key);
                }
                continue;
            }
            rows.sort_by_key(|row| row.played_at);
            state
                .meta
                .names
                .entry(key.clone())
                .or_insert_with(|| user.trim().to_string());
            self.rewrite(&key, rows);
            copied += added;
        }
        state.meta.backfilled = true;
        self.save_meta(&state.meta);
        copied
    }

    /// Removes plays older than the days kept. Returns how many went.
    pub fn prune(&self) -> usize {
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        self.prune_locked(state)
    }

    fn prune_locked(&self, state: &mut Loaded) -> usize {
        let since = self.now() - self.keep();
        let mut removed = 0;
        let mut emptied = Vec::new();
        for (key, rows) in &mut state.users {
            let old = rows.partition_point(|row| row.played_at < since);
            if old == 0 {
                continue;
            }
            rows.drain(..old);
            removed += old;
            if rows.is_empty() {
                emptied.push(key.clone());
                let _ = std::fs::remove_file(self.user_path(key));
            } else {
                self.rewrite(key, rows);
            }
        }
        for key in emptied {
            state.users.remove(&key);
        }
        removed
    }

    /// The genre already found for an artist: Some(None) when it is known to have none.
    pub fn artist_genre(&self, artist: &str) -> Option<Option<String>> {
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        state
            .meta
            .artist_genres
            .get(&artist_key(artist))
            .map(|g| (!g.is_empty()).then(|| g.clone()))
    }

    pub fn remember_artist_genre(&self, artist: &str, genre: Option<&str>) {
        let key = artist_key(artist);
        if key.is_empty() {
            return;
        }
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        state
            .meta
            .artist_genres
            .insert(key, genre.unwrap_or("").to_string());
        self.save_meta(&state.meta);
    }

    /// The genre already read for a library album: Some(None) when it has none.
    pub fn album_genre(&self, album_id: &str) -> Option<Option<String>> {
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        state
            .meta
            .album_genres
            .get(album_id)
            .map(|g| (!g.is_empty()).then(|| g.clone()))
    }

    pub fn remember_album_genre(&self, album_id: &str, genre: Option<&str>) {
        if album_id.is_empty() {
            return;
        }
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        state
            .meta
            .album_genres
            .insert(album_id.to_string(), genre.unwrap_or("").to_string());
        self.save_meta(&state.meta);
    }

    /// The artists of copied-in plays that have no genre yet and no answer kept.
    pub fn artists_missing_genres(&self) -> Vec<String> {
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        let mut seen = std::collections::HashSet::new();
        let mut artists = Vec::new();
        for rows in state.users.values() {
            for row in rows.iter().filter(|row| row.genre.is_none()) {
                let key = artist_key(&row.artist);
                if !state.meta.artist_genres.contains_key(&key) && seen.insert(key) {
                    artists.push(row.artist.clone());
                }
            }
        }
        artists
    }

    /// Gives every play with no genre the genre kept for its artist. Returns how many changed.
    pub fn fill_genres_from_artists(&self) -> usize {
        let mut guard = self.state.lock();
        let state = self.load_locked(&mut guard);
        let mut changed = 0;
        let genres = state.meta.artist_genres.clone();
        let keys: Vec<String> = state.users.keys().cloned().collect();
        for key in keys {
            let Some(rows) = state.users.get_mut(&key) else {
                continue;
            };
            let mut touched = false;
            for row in rows.iter_mut().filter(|row| row.genre.is_none()) {
                if let Some(genre) = genres.get(&artist_key(&row.artist)).filter(|g| !g.is_empty()) {
                    row.genre = Some(genre.clone());
                    touched = true;
                    changed += 1;
                }
            }
            if touched && let Some(rows) = state.users.get(&key) {
                self.rewrite(&key, rows);
            }
        }
        changed
    }

    /// The daily work: the radio history copied in once, the genres of those plays looked up
    /// by artist on Last.fm, then old plays pruned once a day.
    pub async fn run(
        self: Arc<Self>,
        history: Arc<crate::services::last_fm::last_fm_radio_state_store::LastFmRadioStateStore>,
        last_fm: Arc<crate::services::last_fm::LastFmService>,
        stopping: CancellationToken,
    ) -> anyhow::Result<()> {
        let log = Arc::clone(&self);
        let copied = tokio::task::spawn_blocking(move || {
            let people: Vec<(String, Vec<LastFmRadioPlay>)> = history
                .known_users()
                .into_iter()
                .map(|user| {
                    let plays = history.get_user(&user).plays;
                    (user, plays)
                })
                .collect();
            log.backfill(people)
        })
        .await
        .unwrap_or(0);
        if copied > 0 {
            info!("Listening: copied {copied} plays from the radio history");
        }
        for artist in self.artists_missing_genres() {
            if stopping.is_cancelled() || !last_fm.has_api_key() {
                break;
            }
            let genre = super::genres::artist_genre_from_last_fm(&last_fm, &artist).await;
            self.remember_artist_genre(&artist, genre.as_deref());
            tokio::select! {
                () = stopping.cancelled() => return Ok(()),
                () = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
        }
        let filled = self.fill_genres_from_artists();
        if filled > 0 {
            debug!("Listening: {filled} copied plays took their artist's genre");
        }
        loop {
            let removed = self.prune();
            if removed > 0 {
                info!("Listening: removed {removed} plays older than the days kept");
            }
            tokio::select! {
                () = stopping.cancelled() => return Ok(()),
                () = tokio::time::sleep(Duration::from_secs(24 * 3600)) => {}
            }
        }
    }

    // ---- files ----------------------------------------------------------------------------

    fn user_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", file_stem(key)))
    }

    fn append(&self, key: &str, line: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.user_path(key))?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()
    }

    fn rewrite(&self, key: &str, rows: &[Play]) {
        let mut text = String::new();
        for row in rows {
            if let Ok(line) = serde_json::to_string(row) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        if let Err(e) = state_file::save_atomic(&self.user_path(key), &text) {
            warn!("The listening log for {key} could not be written: {e}");
        }
    }

    fn save_meta(&self, meta: &Meta) {
        let mut meta = meta.clone();
        meta.version = 1;
        match serde_json::to_string_pretty(&meta) {
            Ok(text) => {
                if let Err(e) = state_file::save_atomic(&self.dir.join(META_FILE), &text) {
                    warn!("{META_FILE} could not be written: {e}");
                }
            }
            Err(e) => warn!("{META_FILE} could not be written: {e}"),
        }
    }

    fn load_locked<'a>(&self, guard: &'a mut Option<Loaded>) -> &'a mut Loaded {
        if guard.is_none() {
            let mut loaded = Loaded::default();
            if let Ok(Some(text)) = state_file::read_text(&self.dir.join(META_FILE)) {
                match serde_json::from_str::<Meta>(&text) {
                    Ok(meta) => loaded.meta = meta,
                    Err(e) => warn!("{META_FILE} could not be read, starting over: {e}"),
                }
            }
            if let Ok(entries) = std::fs::read_dir(&self.dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let Some(stem) = name.strip_suffix(".jsonl") else {
                        continue;
                    };
                    let Some(key) = key_of_stem(stem) else { continue };
                    let Ok(Some(text)) = state_file::read_text(&entry.path()) else {
                        continue;
                    };
                    // A line torn by a crash mid-append is left out.
                    let mut rows: Vec<Play> = text
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .filter_map(|line| serde_json::from_str(line).ok())
                        .collect();
                    rows.sort_by_key(|row| row.played_at);
                    if !rows.is_empty() {
                        loaded.users.insert(key, rows);
                    }
                }
            }
            *guard = Some(loaded);
            let state = guard.as_mut().expect("just loaded");
            self.prune_locked(state);
        }
        guard.as_mut().expect("loaded")
    }
}

/// How an artist is looked up in the genre answers: trimmed and lower-cased.
fn artist_key(artist: &str) -> String {
    artist.trim().to_lowercase()
}

/// A person's key as a file name: letters, digits, `.`, `_` and `-` as they are, every other
/// byte as `%XX`.
fn file_stem(key: &str) -> String {
    let mut stem = String::new();
    for byte in key.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') || (byte == b'.' && !stem.is_empty()) {
            stem.push(byte as char);
        } else {
            stem.push_str(&format!("%{byte:02X}"));
        }
    }
    stem
}

/// The key a file name stands for.
fn key_of_stem(stem: &str) -> Option<String> {
    let bytes = stem.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = stem.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let key = String::from_utf8(out).ok()?;
    (!key.is_empty()).then_some(key)
}

#[cfg(test)]
#[path = "listening_log_tests.rs"]
mod tests;
