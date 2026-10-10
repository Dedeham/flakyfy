//! Port of `Services/Imports/ImportMatcher.cs`, with `LibraryIndex`.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use futures::future::BoxFuture;
use octo_core::common::Clock;
use octo_core::common::dotnet::is_blank;
use octo_core::common::song_identity::{SongIdentity, SongMatchOptions, SongRef};
use parking_lot::Mutex;
use tracing::{debug, warn};

use super::ImportTrack;
use crate::services::family::library_index::row_full_path;
use crate::services::family::{LibraryScope, ScopeResolver};
use crate::services::library::NavidromePlaylistApi;
use crate::services::library::duplicate_scan_worker::is_lossless_file;
use crate::services::library::library_ownership::LibraryOwnership;
use crate::services::library::quality_upgrade_worker::LibrarySongRow;

/// One page of the library; None when Navidrome could not be read. Tests answer from a list.
pub type ReadPage =
    Arc<dyn Fn(i32, i32) -> BoxFuture<'static, Option<(Vec<LibrarySongRow>, usize)>> + Send + Sync>;

/// Whether the library already has each imported song, the way Octo decides it everywhere else:
/// one MatchKey (the primary artist and the title, one version, so a live take or a remix is its
/// own song) and a length within LibraryOwnership's 8 seconds when both lengths are known. A song
/// the key misses gets a second look through SongIdentity.Same on its title alone, which forgives
/// a credit written another way ("A, B" against "B feat. A", an accent, a renamed artist).
///
/// The whole library is read once, through Navidrome's own song list, and kept a few minutes, so
/// a list of thousands is matched in one pass rather than a search per song. With Family on, a
/// person's list is matched only against the songs they can see: the shared library and their
/// own, never another member's.
pub struct ImportMatcher {
    gate: tokio::sync::Mutex<()>,
    index: Mutex<Option<(Arc<LibraryIndex>, DateTime<Utc>)>>,
    problem: Mutex<Option<String>>,
    read_page: Mutex<ReadPage>,
    clock: Mutex<Clock>,
    scope: Option<ScopeResolver>,
}

impl ImportMatcher {
    pub const KEEP_INDEX_FOR: TimeDelta = TimeDelta::minutes(10);
    const PAGE_SIZE: i32 = 1000;

    pub const CANNOT_READ_LIBRARY: &str = "Octo could not read your Navidrome library, so it cannot tell which songs you have. Octo needs a Navidrome admin sign-in, set under Music server.";

    /// Over Navidrome's song list.
    pub fn new(navidrome: Arc<NavidromePlaylistApi>) -> Self {
        Self::with_pages(Arc::new(move |start, count| {
            let navidrome = navidrome.clone();
            Box::pin(async move { navidrome.list_songs(start, count).await })
        }))
    }

    /// Over any source of pages.
    pub fn with_pages(read_page: ReadPage) -> Self {
        ImportMatcher {
            gate: tokio::sync::Mutex::new(()),
            index: Mutex::new(None),
            problem: Mutex::new(None),
            read_page: Mutex::new(read_page),
            clock: Mutex::new(Clock::system()),
            scope: None,
        }
    }

    /// Matches each person's lists against their own scope of the library.
    pub fn with_library_scope(mut self, scope: ScopeResolver) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn set_read_page(&self, read_page: ReadPage) {
        *self.read_page.lock() = read_page;
    }

    pub fn set_clock(&self, clock: Clock) {
        *self.clock.lock() = clock;
    }

    fn now(&self) -> DateTime<Utc> {
        self.clock.lock().now()
    }

    /// Why the last match could not read the library, in words, or None when it could.
    pub fn problem(&self) -> Option<String> {
        self.problem.lock().clone()
    }

    /// Read the library again on the next match: something was just added to it.
    pub fn forget(&self) {
        *self.index.lock() = None;
    }

    /// Sets each track's LibraryId to the library's song for it, or None. False when the library
    /// could not be read, in which case the tracks keep what an earlier match found.
    pub async fn match_tracks(&self, tracks: &mut [ImportTrack]) -> bool {
        let Some(index) = self.library_index().await else {
            return false;
        };
        for track in tracks.iter_mut() {
            track.library_id = index.find(track).map(|row| row.id.clone());
        }
        true
    }

    /// [`Self::match_tracks`] for one person's lists: only songs that person can see count as
    /// theirs, so a song in another member's own library is still missing for them.
    pub async fn match_tracks_for(&self, owner: &str, tracks: &mut [ImportTrack]) -> bool {
        let Some(index) = self.library_index().await else {
            return false;
        };
        let view = self
            .scope
            .as_ref()
            .map(ScopeResolver::view)
            .filter(|view| view.apart());
        let Some(view) = view else {
            for track in tracks.iter_mut() {
                track.library_id = index.find(track).map(|row| row.id.clone());
            }
            return true;
        };
        let scope = LibraryScope::Viewer(owner.to_string());
        let visible = |row: &LibrarySongRow| {
            let full = row_full_path(row.library_path.as_deref(), &row.path);
            view.allows(&scope, Some(&row.id), Some(&full))
        };
        for track in tracks.iter_mut() {
            track.library_id = index.find_where(track, &visible).map(|row| row.id.clone());
        }
        true
    }

    async fn library_index(&self) -> Option<Arc<LibraryIndex>> {
        let _gate = self.gate.lock().await;
        if let Some((fresh, at)) = self.index.lock().clone()
            && self.now() - at < Self::KEEP_INDEX_FOR
        {
            return Some(fresh);
        }
        let mut rows = Vec::new();
        let mut start = 0;
        loop {
            let read = self.read_page.lock().clone();
            let Some((page, count)) = read(start, Self::PAGE_SIZE).await else {
                warn!("Could not read the library to match imported songs");
                *self.problem.lock() = Some(Self::CANNOT_READ_LIBRARY.to_string());
                return None;
            };
            rows.extend(page);
            if (count as i32) < Self::PAGE_SIZE {
                break;
            }
            start += Self::PAGE_SIZE;
        }
        let count = rows.len();
        let index = Arc::new(LibraryIndex::new(rows));
        *self.index.lock() = Some((index.clone(), self.now()));
        *self.problem.lock() = None;
        debug!("Read {count} library songs to match imported songs against");
        Some(index)
    }
}

/// The library by MatchKey and by title, for matching many songs at once.
pub struct LibraryIndex {
    by_key: HashMap<String, Vec<LibrarySongRow>>,
    by_title: HashMap<String, Vec<LibrarySongRow>>,
}

impl LibraryIndex {
    pub fn new(rows: impl IntoIterator<Item = LibrarySongRow>) -> Self {
        let mut index = LibraryIndex {
            by_key: HashMap::new(),
            by_title: HashMap::new(),
        };
        for row in rows {
            if is_blank(&row.title) {
                continue;
            }
            add(
                &mut index.by_key,
                SongIdentity::match_key(&row.artist, &row.title),
                &row,
            );
            add(
                &mut index.by_title,
                SongIdentity::parse_title(&row.title, Some(&row.artist)).key,
                &row,
            );
        }
        index
    }

    pub fn count(&self) -> usize {
        self.by_key.values().map(Vec::len).sum()
    }

    /// The library's best copy of this song, or None.
    pub fn find(&self, track: &ImportTrack) -> Option<&LibrarySongRow> {
        self.find_where(track, &|_| true)
    }

    /// The best copy among the songs `visible` keeps, or None.
    pub fn find_where(
        &self,
        track: &ImportTrack,
        visible: &dyn Fn(&LibrarySongRow) -> bool,
    ) -> Option<&LibrarySongRow> {
        if let Some(same) = self
            .by_key
            .get(&SongIdentity::match_key(&track.artist, &track.title))
            && let Some(exact) = best(
                same.iter()
                    .filter(|row| visible(row))
                    .filter(|row| length_fits(track.seconds, row.duration)),
                track,
            )
        {
            return Some(exact);
        }

        let title = SongIdentity::parse_title(&track.title, Some(&track.artist)).key;
        if title.is_empty() {
            return None;
        }
        let titled = self.by_title.get(&title)?;
        let wanted = SongRef::new(track.title.clone(), track.artist.clone())
            .with_seconds(track.seconds.map(f64::from));
        let lenient = SongMatchOptions {
            length_tolerance_seconds: Some(LibraryOwnership::DURATION_TOLERANCE_SECONDS),
            ..Default::default()
        };
        best(
            titled.iter().filter(|row| visible(row)).filter(|row| {
                let theirs = SongRef::new(row.title.clone(), row.artist.clone())
                    .with_seconds(row.duration.map(f64::from));
                SongIdentity::same(&wanted, &theirs, Some(&lenient)).is_same()
            }),
            track,
        )
    }
}

fn add(map: &mut HashMap<String, Vec<LibrarySongRow>>, key: String, row: &LibrarySongRow) {
    if key.is_empty() {
        return;
    }
    map.entry(key).or_default().push(row.clone());
}

fn length_fits(wanted: Option<i32>, found: Option<i32>) -> bool {
    match (wanted.filter(|w| *w > 0), found.filter(|f| *f > 0)) {
        (Some(wanted), Some(found)) => (wanted - found).abs() <= LibraryOwnership::DURATION_TOLERANCE_SECONDS,
        _ => true,
    }
}

/// The closest in length, then the same album, then lossless, then the highest bitrate.
fn best<'a>(
    rows: impl Iterator<Item = &'a LibrarySongRow>,
    track: &ImportTrack,
) -> Option<&'a LibrarySongRow> {
    let album_key = track.album.as_deref().map(SongIdentity::key);
    let mut rows: Vec<&LibrarySongRow> = rows.collect();
    // Stable, as OrderBy and its ThenBys are.
    rows.sort_by(|a, b| {
        let distance = |row: &LibrarySongRow| match (
            track.seconds.filter(|s| *s > 0),
            row.duration.filter(|d| *d > 0),
        ) {
            (Some(wanted), Some(found)) => (wanted - found).abs(),
            _ => 0,
        };
        let same_album = |row: &LibrarySongRow| {
            album_key
                .as_ref()
                .is_some_and(|key| SongIdentity::key(row.album.as_deref().unwrap_or("")) == *key)
        };
        distance(a)
            .cmp(&distance(b))
            .then_with(|| same_album(b).cmp(&same_album(a)))
            .then_with(|| {
                is_lossless_file(&b.suffix, b.bit_rate).cmp(&is_lossless_file(&a.suffix, a.bit_rate))
            })
            .then_with(|| b.bit_rate.cmp(&a.bit_rate))
    });
    rows.into_iter().next()
}

#[cfg(test)]
#[path = "import_matcher_tests.rs"]
mod tests;
