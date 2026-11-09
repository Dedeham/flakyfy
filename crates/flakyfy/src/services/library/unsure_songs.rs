//! The songs the one-time YouTube check could not settle, kept so a person can hear them next to
//! their YouTube video and say what they are, and what those answers teach.
//!
//! An answer is a person's. A rule is what enough unanimous answers about one kind of doubt
//! (a bucket: why it is unsure, where the video came from, how far apart they sounded) suggest,
//! and it does nothing until someone accepts it. A rule's own decisions never count as answers,
//! so a rule cannot teach itself, and one person disagreeing with a rule turns it off.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use flakyfy_core::json::datetime;
use flakyfy_core::models::domain::Song;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::services::state_file;

/// Unanimous answers a bucket needs before a rule that settles or asks is suggested.
pub const MIN_ANSWERS_TO_SUGGEST: usize = 10;
/// Unanimous "wrong" answers a bucket needs before a rule that may replace is suggested.
pub const MIN_ANSWERS_TO_SUGGEST_REPLACING: usize = 50;

/// What a person, or a rule, says a song is. The bench's labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsureLabel {
    /// The file is the song its tags name.
    Right,
    /// The file is another song.
    Wrong,
    /// The song, but another recording of it.
    Version,
    /// The video is not the song: it says nothing about the file.
    PreviewWrong,
    /// Heard, and still could not say.
    CantTell,
}

/// Why the check could not say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsureReason {
    /// A video that names the song, but sounding neither alike nor clearly different.
    Between,
    /// A video too long or too short to be believed.
    Length,
    /// A video whose title or uploader does not name the song.
    Name,
}

/// One YouTube video the file was compared with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeardVideo {
    pub video_id: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub seconds: Option<i32>,
    /// The share of bits that differ at the best stretch, 0 to 1.
    pub bit_error: f64,
    /// Where the file's start sits in the video, in seconds.
    pub offset_seconds: f64,
    pub trusted: bool,
    pub reason: UnsureReason,
    /// A person said this video is not the song.
    #[serde(default)]
    pub rejected: bool,
}

impl HeardVideo {
    /// The kind of doubt this comparison is. A row without an album counts as a search hit: a
    /// row's album is what tells the two apart once they are stored.
    pub fn bucket(&self) -> String {
        let source = if self.album.is_some() { "row" } else { "search" };
        let band = ((self.bit_error * 20.0).floor() as i32) * 5;
        format!("{}|{source}|{band}", reason_name(self.reason))
    }
}

fn reason_name(reason: UnsureReason) -> &'static str {
    match reason {
        UnsureReason::Between => "between",
        UnsureReason::Length => "length",
        UnsureReason::Name => "name",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsureDecision {
    pub label: UnsureLabel,
    /// Who said so; empty when a rule did.
    pub by: String,
    /// The rule that said so, when one did.
    pub rule: Option<String>,
    #[serde(with = "datetime::utc")]
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsureSong {
    pub id: u64,
    /// Library-relative, with `/`.
    pub path: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub seconds: Option<i32>,
    /// The file's size and last write when it was compared. A file that changes is open again.
    pub stamp: String,
    pub videos: Vec<HeardVideo>,
    pub decision: Option<UnsureDecision>,
    #[serde(with = "datetime::utc")]
    pub seen: DateTime<Utc>,
}

impl UnsureSong {
    /// What the song's answer is filed under: its first video nobody turned down.
    pub fn bucket(&self) -> Option<String> {
        self.videos
            .iter()
            .find(|video| !video.rejected)
            .or(self.videos.first())
            .map(HeardVideo::bucket)
    }

    /// Answered for good: "can't tell" leaves it open.
    pub fn settled(&self) -> bool {
        self.decision
            .as_ref()
            .is_some_and(|decision| decision.label != UnsureLabel::CantTell)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// Songs of the bucket are taken as right.
    Settle,
    /// Songs of the bucket become a Review question.
    Ask,
    /// Songs of the bucket may be replaced, behind every other gate.
    Replace,
}

impl RuleKind {
    fn label(self) -> UnsureLabel {
        match self {
            RuleKind::Settle => UnsureLabel::Right,
            RuleKind::Ask | RuleKind::Replace => UnsureLabel::Wrong,
        }
    }

    fn name(self) -> &'static str {
        match self {
            RuleKind::Settle => "settle",
            RuleKind::Ask => "ask",
            RuleKind::Replace => "replace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleState {
    Active,
    /// A person turned it down, or off.
    Rejected,
    /// A person disagreed with what it decided.
    Contradicted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsureRule {
    pub id: String,
    pub bucket: String,
    pub kind: RuleKind,
    pub state: RuleState,
    #[serde(with = "datetime::utc")]
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsureState {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub songs: Vec<UnsureSong>,
    #[serde(default)]
    pub rules: Vec<UnsureRule>,
}

/// One bucket's answers from people.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketReport {
    pub bucket: String,
    pub right: usize,
    pub wrong: usize,
    pub version: usize,
    /// Videos people turned down.
    pub video_wrong: usize,
}

impl BucketReport {
    fn answers(&self) -> usize {
        self.right + self.wrong + self.version
    }
}

/// A rule the answers would allow, waiting for a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSuggestion {
    pub id: String,
    pub bucket: String,
    pub kind: RuleKind,
    pub answers: usize,
}

/// What a newly heard song's bucket asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    /// No rule: it waits for a person.
    Wait,
    /// A rule took it as right.
    Settled,
    Ask,
    Replace,
}

/// What each bucket's answers from people came to. A rule's decisions are not answers.
pub fn report(state: &UnsureState) -> Vec<BucketReport> {
    let mut buckets: BTreeMap<String, BucketReport> = BTreeMap::new();
    for song in &state.songs {
        let Some(bucket) = song.bucket() else {
            continue;
        };
        let row = buckets.entry(bucket.clone()).or_insert_with(|| BucketReport {
            bucket,
            ..Default::default()
        });
        row.video_wrong += song.videos.iter().filter(|video| video.rejected).count();
        match song.decision.as_ref().filter(|decision| decision.rule.is_none()) {
            Some(decision) if decision.label == UnsureLabel::Right => row.right += 1,
            Some(decision) if decision.label == UnsureLabel::Wrong => row.wrong += 1,
            Some(decision) if decision.label == UnsureLabel::Version => row.version += 1,
            _ => {}
        }
    }
    buckets.into_values().collect()
}

/// The rules the answers allow and nobody has ruled on yet: only where every answer agrees.
pub fn suggestions(state: &UnsureState) -> Vec<RuleSuggestion> {
    let mut found = Vec::new();
    for row in report(state) {
        let answers = row.answers();
        let mut suggest = |kind: RuleKind| {
            let id = format!("{}|{}", row.bucket, kind.name());
            if !state.rules.iter().any(|rule| rule.id == id) {
                found.push(RuleSuggestion {
                    id,
                    bucket: row.bucket.clone(),
                    kind,
                    answers,
                });
            }
        };
        if answers >= MIN_ANSWERS_TO_SUGGEST && row.right == answers {
            suggest(RuleKind::Settle);
        }
        if answers >= MIN_ANSWERS_TO_SUGGEST && row.wrong == answers {
            suggest(RuleKind::Ask);
        }
        if answers >= MIN_ANSWERS_TO_SUGGEST_REPLACING && row.wrong == answers {
            suggest(RuleKind::Replace);
        }
    }
    found
}

impl UnsureState {
    /// The strongest active rule for `bucket`.
    fn rule_for(&self, bucket: &str) -> Option<&UnsureRule> {
        let rank = |kind: RuleKind| match kind {
            RuleKind::Settle => 0,
            RuleKind::Ask => 1,
            RuleKind::Replace => 2,
        };
        self.rules
            .iter()
            .filter(|rule| rule.state == RuleState::Active && rule.bucket == bucket)
            .max_by_key(|rule| rank(rule.kind))
    }

    /// Turns `id` off because someone disagreed, and opens again what it decided.
    fn contradict(&mut self, id: &str, now: DateTime<Utc>) {
        if let Some(rule) = self
            .rules
            .iter_mut()
            .find(|rule| rule.id == id && rule.state == RuleState::Active)
        {
            rule.state = RuleState::Contradicted;
            rule.at = now;
        }
        for song in &mut self.songs {
            if song
                .decision
                .as_ref()
                .is_some_and(|d| d.rule.as_deref() == Some(id))
            {
                song.decision = None;
            }
        }
    }
}

struct StoreInner {
    state: UnsureState,
    dirty: bool,
}

/// youtube-unsure.json beside the settings, written every few seconds and at once after an answer.
pub struct UnsureStore {
    path: Option<PathBuf>,
    inner: Mutex<StoreInner>,
    flush_lock: Mutex<()>,
}

impl UnsureStore {
    pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);

    /// A store kept in `path` (None keeps it in memory). A file that cannot be read is set aside
    /// as `<file>.corrupt-<ticks>` and the list starts over.
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut state = UnsureState::default();
        if let Some(path) = &path
            && path.exists()
        {
            match Self::load(path) {
                Ok(loaded) => state = loaded,
                Err(e) => {
                    warn!("The list of unsure songs could not be read ({e}); starting over");
                    super::notice_queue::set_aside(path);
                }
            }
        }
        UnsureStore {
            path,
            inner: Mutex::new(StoreInner { state, dirty: false }),
            flush_lock: Mutex::new(()),
        }
    }

    fn load(path: &Path) -> anyhow::Result<UnsureState> {
        let text = state_file::read_all_text(path)?;
        Ok(serde_json::from_str::<Option<UnsureState>>(&text)?.unwrap_or_default())
    }

    pub fn read<T>(&self, read: impl FnOnce(&UnsureState) -> T) -> T {
        read(&self.inner.lock().state)
    }

    fn update<T>(&self, change: impl FnOnce(&mut UnsureState) -> T) -> T {
        let mut inner = self.inner.lock();
        inner.dirty = true;
        change(&mut inner.state)
    }

    /// Writes the list if it changed; see `YoutubeCheckStore::flush`.
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
            flakyfy_core::json::to_string(&inner.state)
        };
        match state_file::save_atomic(path, &json) {
            Ok(()) => true,
            Err(e) => {
                self.inner.lock().dirty = true;
                warn!("The list of unsure songs could not be written: {e}");
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

    /// Files whose answer still holds while their stamp does, by library-relative path.
    pub fn settled_stamps(&self) -> HashMap<String, String> {
        self.read(|state| {
            state
                .songs
                .iter()
                .filter(|song| song.settled())
                .map(|song| (song.path.clone(), song.stamp.clone()))
                .collect()
        })
    }

    /// Keeps what the check heard of the song at `path`, and says what a rule makes of it. A file
    /// that changed since is a new question; an answer on an unchanged file is left alone.
    pub fn record(
        &self,
        path: &str,
        song: &Song,
        stamp: &str,
        video: HeardVideo,
        now: DateTime<Utc>,
    ) -> RuleAction {
        self.update(|state| {
            let rule = state
                .rule_for(&video.bucket())
                .map(|rule| (rule.id.clone(), rule.kind));
            let index = match state.songs.iter().position(|kept| kept.path == path) {
                Some(index) => index,
                None => {
                    let id = state.next_id + 1;
                    state.next_id = id;
                    state.songs.push(UnsureSong {
                        id,
                        path: path.to_string(),
                        artist: song.artist.clone(),
                        title: song.title.clone(),
                        album: song.album.clone(),
                        seconds: song.duration,
                        stamp: stamp.to_string(),
                        videos: Vec::new(),
                        decision: None,
                        seen: now,
                    });
                    state.songs.len() - 1
                }
            };
            let kept = &mut state.songs[index];
            if kept.stamp != stamp {
                kept.stamp = stamp.to_string();
                kept.videos.clear();
                kept.decision = None;
            }
            put_video(kept, video);
            if kept.decision.is_some() {
                return RuleAction::Wait;
            }
            let Some((id, kind)) = rule else {
                return RuleAction::Wait;
            };
            kept.decision = Some(UnsureDecision {
                label: kind.label(),
                by: String::new(),
                rule: Some(id),
                at: now,
            });
            match kind {
                RuleKind::Settle => RuleAction::Settled,
                RuleKind::Ask => RuleAction::Ask,
                RuleKind::Replace => RuleAction::Replace,
            }
        })
    }

    /// Another video heard for the song `id`.
    pub fn add_video(&self, id: u64, video: HeardVideo) {
        self.update(|state| {
            if let Some(song) = state.songs.iter_mut().find(|song| song.id == id) {
                put_video(song, video);
            }
        });
        self.flush();
    }

    pub fn song(&self, id: u64) -> Option<UnsureSong> {
        self.read(|state| state.songs.iter().find(|song| song.id == id).cloned())
    }

    /// The song kept for `path`.
    pub fn song_at(&self, path: &str) -> Option<UnsureSong> {
        self.read(|state| state.songs.iter().find(|song| song.path == path).cloned())
    }

    /// A person's answer about song `id`, or None to take one back. "The video is wrong" turns
    /// the video down and leaves the song open. An answer against an active rule turns it off.
    /// The song as it now stands, None when there is no such song.
    pub fn decide(
        &self,
        id: u64,
        label: Option<UnsureLabel>,
        video_id: Option<&str>,
        by: &str,
        now: DateTime<Utc>,
    ) -> Option<UnsureSong> {
        let answered = self.update(|state| {
            let index = state.songs.iter().position(|song| song.id == id)?;
            if label == Some(UnsureLabel::PreviewWrong) {
                let song = &mut state.songs[index];
                let video = song
                    .videos
                    .iter_mut()
                    .find(|video| !video.rejected && video_id.is_none_or(|id| id == video.video_id))?;
                video.rejected = true;
                song.decision = None;
                return Some(song.clone());
            }
            let against = label
                .filter(|label| *label != UnsureLabel::CantTell)
                .map(|label| {
                    let song = &state.songs[index];
                    let by_rule = song.decision.as_ref().and_then(|d| d.rule.clone());
                    let of_bucket = song
                        .bucket()
                        .and_then(|bucket| state.rule_for(&bucket))
                        .filter(|rule| rule.kind.label() != label)
                        .map(|rule| rule.id.clone());
                    let overruled = by_rule.filter(|_| {
                        song.decision
                            .as_ref()
                            .is_some_and(|decision| decision.label != label)
                    });
                    (overruled, of_bucket)
                });
            state.songs[index].decision = label.map(|label| UnsureDecision {
                label,
                by: by.to_string(),
                rule: None,
                at: now,
            });
            if let Some((overruled, of_bucket)) = against {
                for rule in [overruled, of_bucket].into_iter().flatten() {
                    state.contradict(&rule, now);
                }
            }
            Some(state.songs[index].clone())
        });
        self.flush();
        answered
    }

    /// Accepts a suggestion that still stands. False when it does not.
    pub fn accept_rule(&self, id: &str, now: DateTime<Utc>) -> bool {
        let accepted = self.update(|state| {
            let Some(found) = suggestions(state).into_iter().find(|s| s.id == id) else {
                return false;
            };
            state.rules.push(UnsureRule {
                id: found.id,
                bucket: found.bucket,
                kind: found.kind,
                state: RuleState::Active,
                at: now,
            });
            true
        });
        self.flush();
        accepted
    }

    /// Turns down a suggestion, or turns off a rule. What a rule already decided stays decided.
    pub fn reject_rule(&self, id: &str, now: DateTime<Utc>) -> bool {
        let rejected = self.update(|state| {
            if let Some(rule) = state.rules.iter_mut().find(|rule| rule.id == id) {
                rule.state = RuleState::Rejected;
                rule.at = now;
                return true;
            }
            let Some(found) = suggestions(state).into_iter().find(|s| s.id == id) else {
                return false;
            };
            state.rules.push(UnsureRule {
                id: found.id,
                bucket: found.bucket,
                kind: found.kind,
                state: RuleState::Rejected,
                at: now,
            });
            true
        });
        self.flush();
        rejected
    }
}

impl Drop for UnsureStore {
    fn drop(&mut self) {
        self.flush();
    }
}

/// The smallest share of differing bits when at least two videos nobody turned down all sound at
/// least `at_least` different from the song, None otherwise.
pub fn videos_disagree(song: &UnsureSong, at_least: f64) -> Option<f64> {
    let heard: Vec<f64> = song
        .videos
        .iter()
        .filter(|video| !video.rejected)
        .map(|video| video.bit_error)
        .collect();
    (heard.len() >= 2 && heard.iter().all(|bits| *bits >= at_least))
        .then(|| heard.iter().copied().fold(f64::INFINITY, f64::min))
}

/// Adds `video`, or brings an earlier comparison with it up to date, keeping whether a person
/// turned it down.
fn put_video(song: &mut UnsureSong, mut video: HeardVideo) {
    match song
        .videos
        .iter_mut()
        .find(|kept| kept.video_id == video.video_id)
    {
        Some(kept) => {
            video.rejected = kept.rejected;
            *kept = video;
        }
        None => song.videos.push(video),
    }
}

#[cfg(test)]
#[path = "unsure_songs_tests.rs"]
mod tests;
